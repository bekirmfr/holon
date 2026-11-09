// The server panel: the routes, clocks, file, mail, feed and queue handlers and actions this flow
// declares (derived from its request, every, on_file, on_mail, on_feed, on_queue and action nodes,
// see derive.ts), a request tester to try the routes, locally or against the flow's deployment,
// "tick now" for a deployed clock, what a deployed feed handler's connection and queue handler's
// listener are doing and what its messages to queues came to, and "run" for a deployed action,
// with what to send it.
// Deploying is the header's job (DeployBar), for the whole flow.

import { useEffect, useState, type CSSProperties } from "react";
import { buildPreimage, type Evaluator, type WUnit, type WValue } from "./holon";
import {
  matchRoute,
  METHODS,
  parseHeaders,
  requestRecord,
  responseOf,
  toJson,
  type EndpointDraft,
  type Method,
} from "./endpoint";
import { feedStates, queueStates, sendRequest, ServerError, tickNow, type FeedState, type HttpResponse, type QueueListening, type QueueSending, type Role } from "./server";
import type { ActionDraft, ClockDraft, FeedDraft, FileDraft, MailDraft, QueueDraft, SocketDraft } from "./canvasOps";
import { ActionRows } from "./ActionRows";

const field: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#e6eefb",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "2px 4px",
};
const btn: CSSProperties = { ...field, color: "#9fb4d4", cursor: "pointer", padding: "2px 8px" };
const lbl: CSSProperties = { width: 52, flexShrink: 0 };

/** A request typed into the tester. */
type TesterRequest = { method: Method; url: string; headers: string; body: string };
/** What came back, and from where. */
type TesterResult = ({ from: "local" | "live" } & HttpResponse) | { error: string };

/** Answer `req` in the browser, as the deployed server would: route it across the draft's
 *  endpoints, evaluate the matched one, and write the response the way data.rs does. State reads
 *  see nothing stored, since no state lives in the browser. */
function answerLocally(ev: Evaluator, endpoints: EndpointDraft[], req: TesterRequest): TesterResult {
  const headers = parseHeaders(req.headers);
  if (typeof headers === "string") return { error: headers };
  const json = (status: number, error: string, extra: [string, string][] = []): TesterResult => ({
    from: "local",
    status,
    headers: [["content-type", "application/json"], ...extra],
    body: JSON.stringify({ error }),
  });
  const routed = matchRoute(endpoints, req.method, req.url);
  if ("status" in routed) {
    return routed.status === 404
      ? json(404, "no route for this path")
      : json(405, "method not allowed for this path", [["allow", routed.allow.join(", ")]]);
  }
  const e = routed.endpoint;
  const request = requestRecord(req.method, e.path, req.url, req.body, headers);
  if (!request) return { error: `"${req.url}" doesn't match ${e.path}` };
  let outputs: (WValue | null)[];
  try {
    outputs = ev.eval(buildPreimage(ev.get_unit(e.hash) as WUnit), [request]) as (WValue | null)[];
  } catch (err) {
    return { error: err instanceof Error ? err.message : String(err) };
  }
  const answer = responseOf(outputs[0]);
  let body = "";
  let contentType: string | null = null;
  if (answer.body?.type === "Text") {
    body = answer.body.value;
    contentType = "text/plain; charset=utf-8";
  } else if (answer.body) {
    const j = toJson(answer.body);
    if (j === null) return json(500, "the response body has no JSON form");
    body = j;
    contentType = "application/json";
  }
  const out = [...answer.headers];
  if (contentType && !out.some(([k]) => k === "content-type")) out.push(["content-type", contentType]);
  return { from: "local", status: answer.status, headers: out, body };
}

/** Pretty-print a JSON body; anything else is shown as it came. */
function showBody(body: string): string {
  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return body;
  }
}

type ToolsPanelProps = {
  open: boolean;
  onOpen: (open: boolean) => void;
  ev: Evaluator;
  /** The routes the canvas declares, each already built into an endpoint unit. */
  endpoints: EndpointDraft[];
  /** The clocks the canvas declares (its `every` nodes' groups). */
  clocks?: ClockDraft[];
  /** The file handlers the canvas declares (its `on_file` nodes' groups). */
  files?: FileDraft[];
  /** The mail handlers the canvas declares (its `on_mail` nodes' groups). */
  mails?: MailDraft[];
  /** The feed handlers the canvas declares (its `on_feed` nodes' groups). */
  feeds?: FeedDraft[];
  /** The queue handlers the canvas declares (its `on_queue` nodes' groups). */
  queues?: QueueDraft[];
  /** The queues its flow sends to, by name. */
  sendsTo?: string[];
  /** The websockets' blocks the canvas declares (its `websocket` nodes' groups). */
  websockets?: SocketDraft[];
  /** The actions the canvas declares (its `action` nodes' groups). */
  actions?: ActionDraft[];
  /** The flow's running deployment, for "tick now" and "run"; empty when it isn't running. */
  tickSlug?: string;
  /** Why a request node's group couldn't become a route. */
  errors: string[];
  /** The flow's server deployment, for "send live"; empty when it isn't deployed. */
  liveSlug: string;
  /** Who is signed in: a viewer sees the actions, never runs one. */
  role?: Role;
  /** The canvas has changes not deployed (an action runs the deployed version). */
  outOfDate?: boolean;
  /** Open view mode on a run an action made. */
  onViewRun?: (seq: number) => void;
};

export function ToolsPanel({
  open,
  onOpen,
  ev,
  endpoints,
  clocks = [],
  files = [],
  mails = [],
  feeds = [],
  queues = [],
  sendsTo = [],
  websockets = [],
  actions = [],
  errors,
  liveSlug,
  tickSlug = "",
  role,
  outOfDate = false,
  onViewRun,
}: ToolsPanelProps) {
  const [serverOpen, setServerOpen] = useState(true);
  const [req, setReq] = useState<TesterRequest>({ method: "GET", url: "/", headers: "", body: "" });
  const [result, setResult] = useState<TesterResult | null>(null);
  const [sending, setSending] = useState(false);
  const [ticked, setTicked] = useState("");
  // What the running deployment's connection to each feed is doing, read every few seconds.
  const [feedState, setFeedState] = useState<FeedState[]>([]);
  const listens = feeds.length > 0;
  useEffect(() => {
    if (!tickSlug || !listens) {
      setFeedState([]);
      return;
    }
    let on = true;
    const read = () =>
      feedStates(tickSlug)
        .then((f) => on && setFeedState(f))
        .catch(() => on && setFeedState([]));
    read();
    const every = setInterval(read, 3000);
    return () => {
      on = false;
      clearInterval(every);
    };
  }, [tickSlug, listens]);
  // What the running deployment's listener to each queue is doing, and what its messages to each
  // came to, read every few seconds.
  const [queueState, setQueueState] = useState<{ listening: QueueListening[]; sending: QueueSending[] }>({ listening: [], sending: [] });
  const queued = queues.length + sendsTo.length > 0;
  useEffect(() => {
    if (!tickSlug || !queued) {
      setQueueState({ listening: [], sending: [] });
      return;
    }
    let on = true;
    const read = () =>
      queueStates(tickSlug)
        .then((q) => on && setQueueState(q))
        .catch(() => on && setQueueState({ listening: [], sending: [] }));
    read();
    const every = setInterval(read, 3000);
    return () => {
      on = false;
      clearInterval(every);
    };
  }, [tickSlug, queued]);

  const tick = (clock: string) => {
    setTicked(`${clock}: ticking…`);
    tickNow(tickSlug, clock)
      .then((t) => setTicked(`${clock}: ticked, ${t.performed.length} effect${t.performed.length === 1 ? "" : "s"} performed`))
      .catch((err: unknown) => setTicked(`${clock}: ${err instanceof Error ? err.message : String(err)}`));
  };

  const sendLocal = () => setResult(answerLocally(ev, endpoints, req));

  const sendLive = () => {
    const headers = parseHeaders(req.headers);
    if (typeof headers === "string") {
      setResult({ error: headers });
      return;
    }
    setSending(true);
    sendRequest(liveSlug, { method: req.method, url: req.url, headers, body: req.body })
      .then((r) => setResult({ from: "live", ...r }))
      .catch((err: unknown) => setResult({ error: err instanceof ServerError || err instanceof Error ? err.message : String(err) }))
      .finally(() => setSending(false));
  };

  const canSend = req.url.startsWith("/");

  if (!open) {
    return (
      <aside className="hl-deploy hl-deploy--closed">
        <button
          className="hl-deploy__fold"
          onClick={() => onOpen(true)}
          title="server tools: routes, clocks, files, actions, request tester"
          aria-label="open tools panel"
        >
          tools
        </button>
      </aside>
    );
  }

  return (
    <aside className="hl-deploy" role="complementary" aria-label="Tools">
      <div className="hl-panel-header">
        <span className="hl-panel-header__title">tools</span>
        <button
          className="hl-panel-header__btn"
          onClick={() => onOpen(false)}
          title="hide tools panel"
          aria-label="hide tools panel"
        >›</button>
      </div>
      <div
        style={{
          flex: "1 1 0",
          overflowY: "auto",
          padding: "8px 12px",
          display: "flex",
          flexDirection: "column",
          gap: 6,
          fontSize: 11,
          color: "#9fb4d4",
        }}
      >
        <button
          style={{
            display: "flex", alignItems: "center", width: "100%",
            font: "inherit", background: "transparent", border: "none",
            borderBottom: "1px solid var(--line-soft)",
            color: "var(--ink-dim)", cursor: "pointer", textAlign: "left",
            padding: "0 0 5px", fontSize: 10, letterSpacing: "0.12em",
            textTransform: "uppercase",
          }}
          onClick={() => setServerOpen((v) => !v)}
          aria-expanded={serverOpen}
        >
          server
          <span style={{ fontSize: 9, opacity: 0.5, marginLeft: "auto" }}>{serverOpen ? "▾" : "▸"}</span>
        </button>
        {serverOpen && (
          <>
            {endpoints.length === 0 && clocks.length === 0 && files.length === 0 && mails.length === 0 && feeds.length === 0 && queues.length === 0 && websockets.length === 0 && actions.length === 0 && errors.length === 0 && (
              <div style={{ color: "#3d4f66", fontStyle: "italic" }}>drop a request node (palette, http) and wire it to a respond node to add a route</div>
            )}
            {clocks.map((c) => (
              <div key={c.label} style={{ display: "flex", gap: 6, alignItems: "center" }}>
                <span style={{ color: "#6fd3e0", ...lbl }}>every</span>
                <span style={{ flex: 1, color: "#e6eefb" }}>{c.label.slice("every ".length)}</span>
                <button
                  style={{ ...btn, opacity: tickSlug ? 1 : 0.4 }}
                  disabled={!tickSlug}
                  title={tickSlug ? "run this clock's group once, now, on the deployment" : "deploy the flow to tick it"}
                  onClick={() => tick(c.label)}
                >
                  tick now
                </button>
              </div>
            ))}
            {ticked && <div style={{ color: "#9fb4d4" }}>{ticked}</div>}
            {files.map((f) => (
              <div key={f.label} style={{ display: "flex", gap: 6, alignItems: "center" }} title={`runs its group for each file its line is for, in the folder ${f.folder} on the host`}>
                <span style={{ color: "#6fd3e0", ...lbl }}>on file</span>
                <span style={{ flex: 1, color: "#e6eefb" }}>{f.label.slice("on file ".length)}</span>
              </div>
            ))}
            {mails.map((m) => (
              <div key={m.label} style={{ display: "flex", gap: 6, alignItems: "center" }} title={`runs its group for each message that comes to the mailbox of ${m.mail}, then moves it`}>
                <span style={{ color: "#6fd3e0", ...lbl }}>on mail</span>
                <span style={{ flex: 1, color: "#e6eefb" }}>{m.mail}</span>
              </div>
            ))}
            {feeds.map((f) => {
              const live = feedState.find((s) => s.feed === f.feed);
              return (
                <div key={f.label} style={{ display: "flex", gap: 6, alignItems: "center" }} title={`runs its group for each event the feed ${f.feed} sends, one at a time${live?.address ? ` (${live.address})` : ""}`}>
                  <span style={{ color: "#6fd3e0", ...lbl }}>on feed</span>
                  <span style={{ flex: 1, color: "#e6eefb" }}>{f.feed}</span>
                  {live && <span style={{ color: live.status === "connected" ? "#9fb4d4" : "#e0916f" }}>{feedLine(live)}</span>}
                </div>
              );
            })}
            {queues.map((q) => {
              const live = queueState.listening.find((s) => s.queue === q.queue);
              return (
                <div
                  key={q.label}
                  style={{ display: "flex", gap: 6, alignItems: "center" }}
                  title={`runs its group for each message the queue ${q.queue} holds, one at a time, and deletes it once its run is over${live?.address ? ` (${live.address})` : ""}`}
                >
                  <span style={{ color: "#6fd3e0", ...lbl }}>on queue</span>
                  <span style={{ flex: 1, color: "#e6eefb" }}>{q.queue}</span>
                  {live && <span style={{ color: live.status === "receiving" ? "#9fb4d4" : "#e0916f" }}>{queueLine(live)}</span>}
                </div>
              );
            })}
            {sendsTo.map((name) => {
              const sent = queueState.sending.find((s) => s.queue === name);
              return (
                <div key={`sends ${name}`} style={{ display: "flex", gap: 6, alignItems: "center" }} title={`sends messages to the queue ${name} once each run is over${sent?.address ? ` (${sent.address})` : ""}`}>
                  <span style={{ color: "#6fd3e0", ...lbl }}>sends to</span>
                  <span style={{ flex: 1, color: "#e6eefb" }}>{name}</span>
                  {sent && <span style={{ color: sent.gave_up ? "#e0916f" : "#9fb4d4" }}>{sentLine(sent)}</span>}
                </div>
              );
            })}
            <ActionRows names={actions.map((a) => a.name)} slug={tickSlug} role={role} outOfDate={outOfDate} onViewRun={onViewRun} />
            {[...new Set(websockets.map((w) => w.path))].map((path) => (
              <div
                key={`websocket ${path}`}
                style={{ display: "flex", gap: 6, alignItems: "center" }}
                title="holds the WebSocket connections a page opens on this path, and runs its blocks: open as one opens, message on each text it sends, close once it has ended"
              >
                <span style={{ color: "#6fd3e0", ...lbl }}>websocket</span>
                <span style={{ flex: 1, color: "#e6eefb" }}>{path}</span>
                <span style={{ color: "#9fb4d4" }}>{websockets.filter((w) => w.path === path).map((w) => w.on).join(", ")}</span>
              </div>
            ))}
            {endpoints.map((e) => (
              <div key={`${e.method} ${e.path}`} style={{ display: "flex", gap: 6, alignItems: "center" }}>
                <span style={{ color: "#6fd3e0", ...lbl }}>{e.method}</span>
                <span style={{ flex: 1, color: "#e6eefb" }}>{e.path}</span>
                <button
                  style={btn}
                  title="load this route into the request tester"
                  onClick={() => setReq({ ...req, method: e.method, url: e.path })}
                >
                  test
                </button>
              </div>
            ))}
            {errors.map((e) => (
              <div key={e} style={{ color: "#e0916f" }}>
                {e}
              </div>
            ))}

            {/* ── request tester ── */}
            <div style={{ borderTop: "1px solid #1b2a45", paddingTop: 6, display: "flex", flexDirection: "column", gap: 4 }}>
              <div style={{ color: "#e6eefb", fontSize: 10, textTransform: "uppercase", letterSpacing: "0.12em", marginBottom: 2 }}>request</div>
              <div style={{ display: "flex", gap: 4 }}>
                <select value={req.method} onChange={(e) => setReq({ ...req, method: e.target.value as Method })} style={field}>
                  {METHODS.map((m) => (
                    <option key={m}>{m}</option>
                  ))}
                </select>
                <input
                  value={req.url}
                  onChange={(e) => setReq({ ...req, url: e.target.value })}
                  placeholder="/todos/1?full=1"
                  style={{ ...field, flex: 1, minWidth: 0 }}
                />
              </div>
              <textarea
                value={req.headers}
                onChange={(e) => setReq({ ...req, headers: e.target.value })}
                placeholder={"headers, one per line\ncontent-type: application/json"}
                rows={2}
                style={{ ...field, resize: "vertical" }}
              />
              <textarea
                value={req.body}
                onChange={(e) => setReq({ ...req, body: e.target.value })}
                placeholder={'body, e.g. {"title": "buy milk"}'}
                rows={3}
                style={{ ...field, resize: "vertical" }}
              />
              <div style={{ display: "flex", gap: 6 }}>
                <button
                  style={{ ...btn, opacity: canSend && endpoints.length > 0 ? 1 : 0.4 }}
                  disabled={!canSend || endpoints.length === 0}
                  onClick={sendLocal}
                  title="answer it in the browser, from the draft endpoints (no stored state)"
                >
                  send local
                </button>
                <button
                  style={{ ...btn, opacity: canSend && liveSlug && !sending ? 1 : 0.4 }}
                  disabled={!canSend || !liveSlug || sending}
                  onClick={sendLive}
                  title={liveSlug ? `send it to the deployed server "${liveSlug}"` : "deploy the server (or type its slug) first"}
                >
                  {sending ? "sending…" : "send live"}
                </button>
              </div>
              {result &&
                ("error" in result ? (
                  <div style={{ color: "#e0916f" }}>{result.error}</div>
                ) : (
                  <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                    <div>
                      <span style={{ color: result.status < 400 ? "#9ad29f" : "#e0916f" }}>← {result.status}</span>
                      <span> · {result.from === "live" ? `live (${liveSlug})` : "local"}</span>
                    </div>
                    {result.headers.map(([k, v]) => (
                      <div key={k} style={{ color: "#7f93b3" }}>
                        {k}: {v}
                      </div>
                    ))}
                    {result.body && (
                      <pre style={{ ...field, margin: 0, whiteSpace: "pre-wrap", maxHeight: 160, overflowY: "auto" }}>
                        {showBody(result.body)}
                      </pre>
                    )}
                  </div>
                ))}
            </div>
          </>
        )}
      </div>
    </aside>
  );
}

/** @deprecated Use ToolsPanel. */
/** A feed connection's state as one line: `connected, at BTC-USD/41`, `reconnecting: <why>`, how
 *  many events wait, how many its buffer dropped or replaced and its binding passed over, and how
 *  many numbers the feed skipped (`49 in gaps`). */
export function feedLine(f: FeedState): string {
  const at = f.position ? `, at ${f.position}` : "";
  const counts: [number | undefined, string][] = [
    [f.waiting, "waiting"],
    [f.replaced, "replaced"],
    [f.missed, "missed"],
    [f.passed_over, "passed over"],
    [f.gaps, "in gaps"],
  ];
  const said = counts.map(([n, what]) => (n ? `, ${n} ${what}` : "")).join("");
  return f.status === "reconnecting" && f.error ? `reconnecting: ${f.error}${said}` : `${f.status}${at}${said}`;
}

/** A queue listener's state as one line: `receiving, 4 run, 1 passed over`, `unreachable: <why>`,
 *  how many received wait for their run and how many were left for the queue's redrive. */
export function queueLine(q: QueueListening): string {
  const counts: [number | undefined, string][] = [
    [q.waiting, "waiting"],
    [q.run, "run"],
    [q.passed_over, "passed over"],
    [q.kept, "kept for the redrive"],
  ];
  const said = counts.map(([n, what]) => (n ? `, ${n} ${what}` : "")).join("");
  return q.status === "unreachable" && q.error ? `unreachable: ${q.error}${said}` : `${q.status}${said}`;
}

/** What a deployment's messages to a queue came to, as one line: `2 sent, 1 waiting, 1 given up`. */
export function sentLine(s: QueueSending): string {
  const counts: [number | undefined, string][] = [
    [s.sent, "sent"],
    [s.waiting, "waiting"],
    [s.gave_up, "given up"],
  ];
  return counts.flatMap(([n, what]) => (n ? [`${n} ${what}`] : [])).join(", ") || "none sent yet";
}

export function ServerPanel({
  ev,
  endpoints,
  clocks = [],
  files = [],
  mails = [],
  feeds = [],
  queues = [],
  sendsTo = [],
  websockets = [],
  actions = [],
  errors,
  liveSlug,
  tickSlug = "",
  role,
  outOfDate = false,
  onViewRun,
}: Omit<ToolsPanelProps, "open" | "onOpen">) {
  const [open, setOpen] = useState(true);
  return <ToolsPanel open={open} onOpen={setOpen} ev={ev} endpoints={endpoints} clocks={clocks} files={files} mails={mails} feeds={feeds} queues={queues} sendsTo={sendsTo} websockets={websockets} actions={actions} errors={errors} liveSlug={liveSlug} tickSlug={tickSlug} role={role} outOfDate={outOfDate} onViewRun={onViewRun} />;
}
