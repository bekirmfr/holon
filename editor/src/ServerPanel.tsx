// The server panel (M1): turn the projected handler into an endpoint, collect endpoints into a
// server with a port, test an endpoint locally, and deploy the server. The server and its
// endpoints are ordinary units (core's declare.rs), so deploying one is deploying a hash.

import { useMemo, useState, type CSSProperties } from "react";
import { buildPreimage, type Evaluator, type WCapability, type WPort, type WUnit, type WValue } from "./holon";
import { capabilityAcknowledgeName, formatValue } from "./diagnostics";
import { endpointUnit, METHODS, requestRecord, responseOf, serverUnit, type EndpointDraft, type Method } from "./endpoint";

/** What the panel keeps per flow (saved with it): the server's port and its endpoints. */
export type ServerDraft = { port: string; endpoints: EndpointDraft[] };

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

export function ServerPanel({
  ev,
  handler,
  draft,
  setDraft,
  deploy,
}: {
  ev: Evaluator;
  /** The current projection, a candidate handler; `null` when nothing is projected. */
  handler: { hash: string; ports: WPort[]; outputs: WPort[] } | null;
  draft: ServerDraft;
  setDraft: (d: ServerDraft) => void;
  /** Stage `hash`'s closure and deploy it under `slug`, acknowledging `acknowledge`. */
  deploy: (hash: string, slug: string, acknowledge: string[]) => Promise<void>;
}) {
  const [method, setMethod] = useState<Method>("GET");
  const [path, setPath] = useState("/");
  const [slug, setSlug] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState<{ index: number; url: string; body: string; result: string } | null>(null);

  const portNumber = Number(draft.port);
  const portOk = Number.isInteger(portNumber) && portNumber >= 1 && portNumber <= 65535;

  // The server this draft would deploy, and what it may touch, shown before deploying.
  const server = useMemo(() => {
    if (draft.endpoints.length === 0 || !portOk) return null;
    try {
      const hash = ev.insert(buildPreimage(serverUnit(draft.endpoints.map((e) => e.hash), portNumber)));
      const caps = (ev.capabilities(hash) as WCapability[]).map(capabilityAcknowledgeName);
      return { hash, caps };
    } catch {
      return null;
    }
  }, [draft.endpoints, portOk, portNumber, ev]);

  const addEndpoint = () => {
    if (!handler) return;
    const unit = endpointUnit(handler.hash, handler.ports, handler.outputs, method, path.trim());
    if (typeof unit === "string") {
      setMsg(unit);
      return;
    }
    const hash = ev.insert(buildPreimage(unit));
    const others = draft.endpoints.filter((e) => !(e.method === method && e.path === path.trim()));
    setDraft({ ...draft, endpoints: [...others, { method, path: path.trim(), hash }] });
    setMsg(others.length < draft.endpoints.length ? `replaced ${method} ${path.trim()}` : "");
  };

  const send = () => {
    if (!testing) return;
    const e = draft.endpoints[testing.index];
    const request = requestRecord(e.method, e.path, testing.url, testing.body);
    if (!request) {
      setTesting({ ...testing, result: `"${testing.url}" doesn't match ${e.path}` });
      return;
    }
    try {
      const outputs = ev.eval(buildPreimage(ev.get_unit(e.hash) as WUnit), [request]) as (WValue | null)[];
      const { status, body } = responseOf(outputs[0]);
      setTesting({ ...testing, result: `${status}${body ? ` · ${formatValue(body)}` : ""}` });
    } catch (err) {
      setTesting({ ...testing, result: err instanceof Error ? err.message : String(err) });
    }
  };

  const onDeploy = () => {
    if (!server || !slug.trim()) return;
    setBusy(true);
    setMsg("");
    deploy(server.hash, slug.trim(), server.caps)
      .then(() => setMsg(`deployed — try curl http://127.0.0.1:${portNumber}${draft.endpoints[0]?.path ?? "/"}`))
      .catch((err: unknown) => setMsg(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false));
  };

  return (
    <div
      className="nodrag"
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
        width: 300,
      }}
    >
      <div style={{ color: "#e6eefb", letterSpacing: "0.04em" }}>server</div>

      <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
        <select value={method} onChange={(e) => setMethod(e.target.value as Method)} style={field}>
          {METHODS.map((m) => (
            <option key={m}>{m}</option>
          ))}
        </select>
        <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="/todos/:id" style={{ ...field, flex: 1 }} />
        <button
          onClick={addEndpoint}
          disabled={!handler}
          title={handler ? "make the projected composite this route's handler" : "project a handler first"}
          style={{ ...button, opacity: handler ? 1 : 0.4, cursor: handler ? "pointer" : "default" }}
        >
          add endpoint
        </button>
      </div>

      {draft.endpoints.map((e, i) => (
        <div key={`${e.method} ${e.path}`} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <span style={{ color: "#6fd3e0", width: 52 }}>{e.method}</span>
            <span style={{ flex: 1, color: "#e6eefb" }}>{e.path}</span>
            <button style={button} onClick={() => setTesting({ index: i, url: e.path, body: "", result: "" })}>
              test
            </button>
            <button
              style={button}
              title="remove this endpoint"
              onClick={() => {
                setDraft({ ...draft, endpoints: draft.endpoints.filter((_, j) => j !== i) });
                setTesting(null);
              }}
            >
              ×
            </button>
          </div>
          {testing?.index === i && (
            <div style={{ display: "flex", flexDirection: "column", gap: 4, paddingLeft: 58 }}>
              <input value={testing.url} onChange={(ev2) => setTesting({ ...testing, url: ev2.target.value })} style={field} />
              {e.method !== "GET" && (
                <textarea
                  value={testing.body}
                  onChange={(ev2) => setTesting({ ...testing, body: ev2.target.value })}
                  placeholder="request body"
                  rows={2}
                  style={{ ...field, resize: "vertical" }}
                />
              )}
              <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                <button style={button} onClick={send}>
                  send
                </button>
                <span style={{ color: "#9ad29f" }} title="evaluated in the browser; state reads see nothing stored">
                  {testing.result}
                </span>
              </div>
            </div>
          )}
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
    </div>
  );
}
