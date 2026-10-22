// The pages shown over the canvas: saved flows, and deployments.

import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { deleteFlow, getFlow, listFlows, type DeploymentEntry, type FlowListEntry, type FlowRecord, type TriggerResult } from "./server";
import { openChoice } from "./flowDoc";
import { useDialog } from "./Dialog";
import { DeploymentCard } from "./DeploymentCard";

/** Which top-level view is showing. The canvas is never unmounted (React Flow's own state —
 *  nodes mid-edit, viewport — lives there and must survive navigation); the other pages render
 *  as overlays on top of it. `marketplace` is deliberately absent: deferred, and a menu entry
 *  that can't be picked shouldn't be representable as a page state either. */
export type AppPage = "canvas" | "flows" | "deployed";

/** The saved-flows page: list, open, delete. Publishing to the marketplace is deferred — the
 *  button is real but disabled, honestly labeled, rather than wired to something half-built.
 *  "open" on the flow being edited goes back to its unsaved changes; only "reload saved" (shown
 *  for that flow) loads the saved copy over them. */
export function FlowsPage({
  current,
  onOpen,
  onResume,
}: {
  current: { name: string | null; dirty: boolean };
  onOpen: (rec: FlowRecord) => void;
  onResume: () => void;
}) {
  const dialog = useDialog();
  const [flows, setFlows] = useState<FlowListEntry[]>([]);
  const [msg, setMsg] = useState("");

  const refresh = useCallback(() => {
    listFlows()
      .then((f) => {
        setFlows(f);
        setMsg("");
      })
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);

  const open = (name: string) => {
    if (openChoice(name, current) === "resume") return onResume();
    reload(name);
  };
  const reload = (name: string) => {
    getFlow(name)
      .then(onOpen)
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  };
  const remove = async (name: string) => {
    if (!(await dialog.confirm(`Delete the saved flow "${name}"? This can't be undone.`, "Delete"))) return;
    deleteFlow(name)
      .then(refresh)
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  };

  const smallBtn: CSSProperties = {
    font: "inherit",
    fontSize: 11,
    padding: "2px 8px",
    color: "#9fb4d4",
    background: "#0e1a2f",
    border: "1px solid #1b2a45",
    borderRadius: 3,
    cursor: "pointer",
  };

  return (
    <div style={{ maxWidth: 560 }}>
      <div style={{ color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", marginBottom: 12 }}>
        flows
      </div>
      {msg && <div style={{ color: "#e0916f", fontSize: 12, marginBottom: 8 }}>{msg}</div>}
      {flows.length === 0 && !msg && (
        <div style={{ color: "#9fb4d4", fontSize: 12 }}>
          nothing saved yet — build something on the canvas and hit save
        </div>
      )}
      {flows.map((f) => (
        <div
          key={f.name}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "8px 10px",
            marginBottom: 6,
            background: "#0e1a2f",
            border: "1px solid #1b2a45",
            borderRadius: 4,
          }}
        >
          <span style={{ color: "#6fd3e0", fontSize: 13, flex: 1 }}>{f.name}</span>
          <span style={{ color: "#9fb4d4", fontSize: 11 }}>
            saved {new Date(f.saved_at_epoch_seconds * 1000).toLocaleString()}
          </span>
          {openChoice(f.name, current) === "resume" ? (
            <>
              <span style={{ color: "#e0c36f", fontSize: 11 }}>unsaved changes</span>
              <button style={smallBtn} onClick={() => open(f.name)} title="back to the canvas, changes kept">
                open
              </button>
              <button style={smallBtn} onClick={() => reload(f.name)} title="discard your changes and load the saved copy">
                reload saved
              </button>
            </>
          ) : (
            <button style={smallBtn} onClick={() => open(f.name)}>
              open
            </button>
          )}
          <button
            style={{ ...smallBtn, opacity: 0.4, cursor: "default" }}
            disabled
            title="marketplace is deferred — publishing lands with it"
          >
            publish
          </button>
          <button style={smallBtn} onClick={() => remove(f.name)} title="delete this saved flow">
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

export type TriggerOutcome = { ok: true; result: TriggerResult } | { ok: false; message: string };

/** The deployed page: the server's live table, one card per deployment. */
export function DeployedPage({
  deployments,
  error,
  onAction,
  triggerInputs,
  onTriggerInputChange,
  onTrigger,
  triggerResults,
  onDelete,
  onView,
}: {
  deployments: DeploymentEntry[];
  error: string;
  onAction: (slug: string, action: "run" | "pause" | "stop", from?: "latest" | "fresh" | number) => Promise<void>;
  triggerInputs: Record<string, string>;
  onTriggerInputChange: (slug: string, value: string) => void;
  onTrigger: (slug: string) => void;
  triggerResults: Record<string, TriggerOutcome>;
  onDelete: (slug: string) => void;
  onView: (slug: string) => void;
}) {
  return (
    <div style={{ maxWidth: 760 }}>
      <div style={{ color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", marginBottom: 4 }}>
        deployed
      </div>
      <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 12 }}>
        {deployments.length} deployment{deployments.length === 1 ? "" : "s"} ·{" "}
        {deployments.reduce((a, d) => a + d.trigger_count, 0)} fires ·{" "}
        {deployments.reduce((a, d) => a + d.error_count, 0)} errors
      </div>
      {error && <div style={{ color: "#e0916f", fontSize: 12, marginBottom: 8 }}>{error}</div>}
      {deployments.length === 0 && (
        <div style={{ color: "#9fb4d4", fontSize: 12 }}>
          nothing deployed — project a flow on the canvas, then deploy it
        </div>
      )}
      {deployments.map((entry) => (
        <DeploymentCard
          key={entry.slug}
          entry={entry}
          onAction={(action, from) => onAction(entry.slug, action, from)}
          triggerInput={triggerInputs[entry.slug] ?? ""}
          onTriggerInputChange={(v) => onTriggerInputChange(entry.slug, v)}
          onTrigger={() => onTrigger(entry.slug)}
          onDelete={() => onDelete(entry.slug)}
          onView={() => onView(entry.slug)}
          result={triggerResults[entry.slug]}
        />
      ))}
    </div>
  );
}
