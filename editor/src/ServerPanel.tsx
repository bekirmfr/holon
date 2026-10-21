// The server panel (M1): the routes this flow declares (derived from its request nodes, see
// derive.ts), the port, a request tester, and deploy. The server and its endpoints are ordinary
// units (core's declare.rs), so deploying one is deploying a hash.

import { useMemo, useState, type CSSProperties } from "react";
import { buildPreimage, type Evaluator, type WCapability, type WUnit, type WValue } from "./holon";
import { capabilityAcknowledgeName } from "./diagnostics";
import {
  matchRoute,
  METHODS,
  parseHeaders,
  requestRecord,
  responseOf,
  serverUnit,
  toJson,
  type EndpointDraft,
  type Method,
} from "./endpoint";
import { sendRequest, ServerError, type HttpResponse } from "./server";

/** What the panel keeps per flow (saved with it): the server's port. The routes are derived. */
export type ServerDraft = { port: string };

const field: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#e6eefb",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "2px 4px",
};
const button: CSSProperties = { ...field, color: "#9fb4d4", cursor: "pointer", padding: "2px 8px" };
const label: CSSProperties = { width: 52, flexShrink: 0 };

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

export function ServerPanel({
  ev,
  endpoints,
  errors,
  draft,
  setDraft,
  deploy,
}: {
  ev: Evaluator;
  /** The routes the canvas declares, each already built into an endpoint unit. */
  endpoints: EndpointDraft[];
  /** Why a request node's group couldn't become a route. */
  errors: string[];
  draft: ServerDraft;
  setDraft: (d: ServerDraft) => void;
  /** Stage `hash`'s closure and deploy it under `slug`, acknowledging `acknowledge`. */
  deploy: (hash: string, slug: string, acknowledge: string[]) => Promise<void>;
}) {
  const [slug, setSlug] = useState("");
  const [deployedSlug, setDeployedSlug] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [req, setReq] = useState<TesterRequest>({ method: "GET", url: "/", headers: "", body: "" });
  const [result, setResult] = useState<TesterResult | null>(null);
  const [sending, setSending] = useState(false);
  const [open, setOpen] = useState(true);

  const portNumber = Number(draft.port);
  const portOk = Number.isInteger(portNumber) && portNumber >= 1 && portNumber <= 65535;
  const liveSlug = deployedSlug || slug.trim();

  // The server this draft would deploy, and what it may touch, shown before deploying.
  const server = useMemo(() => {
    if (endpoints.length === 0 || !portOk || errors.length > 0) return null;
    try {
      const hash = ev.insert(buildPreimage(serverUnit(endpoints.map((e) => e.hash), portNumber)));
      const caps = (ev.capabilities(hash) as WCapability[]).map(capabilityAcknowledgeName);
      return { hash, caps };
    } catch {
      return null;
    }
  }, [endpoints, errors, portOk, portNumber, ev]);

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

  const onDeploy = () => {
    if (!server || !slug.trim()) return;
    setBusy(true);
    setMsg("");
    deploy(server.hash, slug.trim(), server.caps)
      .then(() => {
        setDeployedSlug(slug.trim());
        setMsg(`deployed — live at http://127.0.0.1:${portNumber}`);
      })
      .catch((err: unknown) => setMsg(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false));
  };

  const canSend = req.url.startsWith("/");

  return (
    <div
      className="nodrag nowheel"
      style={{
        fontSize: 11,
        color: "#9fb4d4",
        background: "#0b1527",
        border: "1px solid #1b2a45",
        borderRadius: 4,
        padding: 8,
        display: "flex",
        flexDirection: "column",
        gap: 6,
        width: open ? 320 : "auto",
        maxHeight: "70vh",
        overflowY: "auto",
      }}
    >
      <div
        style={{ color: "#e6eefb", letterSpacing: "0.04em", cursor: "pointer", userSelect: "none" }}
        onClick={() => setOpen(!open)}
        title={open ? "collapse" : "expand"}
      >
        server {open ? "▾" : `▸ ${endpoints.length} route${endpoints.length === 1 ? "" : "s"}`}
      </div>
      {open && (
        <>

      {endpoints.length === 0 && errors.length === 0 && (
        <div>drop a request node (palette, http) and wire it to a respond node to add a route</div>
      )}
      {endpoints.map((e) => (
        <div key={`${e.method} ${e.path}`} style={{ display: "flex", gap: 6, alignItems: "center" }}>
          <span style={{ color: "#6fd3e0", ...label }}>{e.method}</span>
          <span style={{ flex: 1, color: "#e6eefb" }}>{e.path}</span>
          <button
            style={button}
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

      <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
        <input
          value={draft.port}
          onChange={(e) => setDraft({ ...draft, port: e.target.value })}
          placeholder="port"
          style={{ ...field, width: 56 }}
        />
        <input value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="slug" style={{ ...field, flex: 1 }} />
        <button
          onClick={onDeploy}
          disabled={!server || !slug.trim() || busy}
          style={{
            ...button,
            color: "#0e1a2f",
            background: "#6fd3e0",
            border: "none",
            opacity: !server || !slug.trim() || busy ? 0.4 : 1,
            cursor: !server || !slug.trim() || busy ? "default" : "pointer",
          }}
        >
          {busy ? "deploying…" : "deploy server"}
        </button>
      </div>
      {server && server.caps.length > 0 && <div>touches: {server.caps.join(", ")}</div>}
      {msg && <div style={{ color: msg.startsWith("deployed") ? "#9ad29f" : "#e0916f" }}>{msg}</div>}

      {/* ── request tester ── */}
      <div style={{ borderTop: "1px solid #1b2a45", paddingTop: 6, display: "flex", flexDirection: "column", gap: 4 }}>
        <div style={{ color: "#e6eefb", letterSpacing: "0.04em" }}>request</div>
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
            style={{ ...field, flex: 1 }}
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
            style={{ ...button, opacity: canSend && endpoints.length > 0 ? 1 : 0.4 }}
            disabled={!canSend || endpoints.length === 0}
            onClick={sendLocal}
            title="answer it in the browser, from the draft endpoints (no stored state)"
          >
            send local
          </button>
          <button
            style={{ ...button, opacity: canSend && liveSlug && !sending ? 1 : 0.4 }}
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
  );
}
