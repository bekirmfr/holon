// The pages shown over the canvas: saved flows, and deployments.

import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { holonUses, type HolonUse } from "./holonUsage";
import { deleteFlow, getAllFlows, getFlow, listFlows, type DeploymentEntry, type FlowDocument, type FlowListEntry, type FlowRecord, type TriggerResult } from "./server";
import { ManifestSection, type ManifestView } from "./Manifest";
import { openChoice } from "./tabs";
import { useDialog } from "./Dialog";
import { DeploymentCard } from "./DeploymentCard";
import type { Sample } from "./samples";

/** Which top-level view is showing. The canvas is never unmounted (React Flow's own state —
 *  nodes mid-edit, viewport — lives there and must survive navigation); the other pages render
 *  as overlays on top of it. `marketplace` is deliberately absent: deferred, and a menu entry
 *  that can't be picked shouldn't be representable as a page state either. */
export type AppPage = "canvas" | "flows" | "deployed";

/** The saved-flows page: list, open, delete. Publishing to the marketplace is deferred — the
 *  button is real but disabled, honestly labeled, rather than wired to something half-built.
 *  "open" on a flow open in a tab goes to that tab, unsaved changes and all; only "reload saved"
 *  (shown when that tab has changes) loads the saved copy over them. */
export function FlowsPage({
  openTabs,
  onOpen,
  onSwitch,
  samples,
  onOpenSample,
  manifestOf,
}: {
  /** The open tabs: a flow open in one is gone back to, not opened again. */
  openTabs: { id: number; name: string | null; dirty: boolean }[];
  /** A saved document's manifest items, derived as deploying it would (App: core's capabilities
   *  plus its deploy settings' requirements). Throws why it can't be derived. */
  manifestOf: (doc: FlowDocument) => ManifestView;
  /** Open a saved flow in a new tab, or (`reload`) over its open tab. */
  onOpen: (rec: FlowRecord, reload?: boolean) => void;
  onSwitch: (tab: number) => void;
  /** Sample flows (samples.ts), one per capability: opening one starts an unsaved copy. */
  samples: Sample[];
  onOpenSample: (sample: Sample) => void;
}) {
  const dialog = useDialog();
  const [flows, setFlows] = useState<FlowListEntry[]>([]);
  const [msg, setMsg] = useState("");
  // Which saved flows place which as a node (holonUsage.ts).
  const [uses, setUses] = useState<Map<string, HolonUse[]>>(new Map());

  const refresh = useCallback(() => {
    listFlows()
      .then((f) => {
        setFlows(f);
        setMsg("");
      })
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
    getAllFlows()
      .then((all) => setUses(holonUses(all)))
      .catch(() => setUses(new Map()));
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);

  const tabOf = (name: string) => openTabs.find((t) => t.name === name);
  const open = (name: string) => {
    const tab = tabOf(name);
    if (openChoice(name, openTabs) === "switch" && tab) return onSwitch(tab.id);
    load(name, false);
  };
  const load = (name: string, reload: boolean) => {
    getFlow(name)
      .then((rec) => onOpen(rec, reload))
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
        <div key={f.name} style={{ padding: "8px 10px", marginBottom: 6, background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 4 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ color: "#6fd3e0", fontSize: 13, flex: 1 }}>{f.name}</span>
            <span style={{ color: "#9fb4d4", fontSize: 11 }}>
              saved {new Date(f.saved_at_epoch_seconds * 1000).toLocaleString()}
              {f.saved_by ? ` by ${f.saved_by}` : ""}
            </span>
            {tabOf(f.name)?.dirty ? (
              <>
                <span style={{ color: "#e0c36f", fontSize: 11 }}>unsaved changes</span>
                <button style={smallBtn} onClick={() => open(f.name)} title="go to its tab, changes kept">
                  open
                </button>
                <button style={smallBtn} onClick={() => load(f.name, true)} title="discard your changes and load the saved copy">
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
          {(uses.get(f.name) ?? []).length > 0 && (
            <div style={{ color: "#7e8ea8", fontSize: 11, marginTop: 4 }} title="saved flows that place this one as a node; each keeps the version it placed until updated (↻ on the node)">
              placed in{" "}
              {uses.get(f.name)!.map((u, i) => (
                <span key={u.user}>
                  {i > 0 && ", "}
                  <span style={{ color: "#cfe3ff" }}>{u.user}</span>
                  {u.nodes > 1 && ` ×${u.nodes}`}
                  {!u.current && <span style={{ color: "#e0c36f" }}> (an older save)</span>}
                </span>
              ))}
            </div>
          )}
          <ManifestSection
            version={String(f.saved_at_epoch_seconds)}
            load={() => getFlow(f.name).then((rec) => manifestOf(rec.document))}
          />
        </div>
      ))}

      <div style={{ color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", margin: "24px 0 4px" }}>samples</div>
      <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 10 }}>
        one per capability; opening one starts an unsaved copy you can change, save and deploy
      </div>
      {samples.map((s) => (
        <div
          key={s.name}
          style={{
            display: "flex",
            alignItems: "flex-start",
            gap: 10,
            padding: "8px 10px",
            marginBottom: 6,
            background: "#0e1a2f",
            border: "1px solid #1b2a45",
            borderRadius: 4,
          }}
        >
          <div style={{ flex: 1 }}>
            <div style={{ color: "#6fd3e0", fontSize: 13 }}>{s.title}</div>
            <div style={{ color: "#cfe3ff", fontSize: 11, margin: "2px 0" }}>{s.description}</div>
            <div style={{ color: "#7e8ea8", fontSize: 10 }}>
              shows {s.shows.join(" · ")}
              {s.secrets && s.secrets.length > 0 && <> · needs secrets {s.secrets.map((x) => (x.from ? `${x.name} (${x.from})` : x.name)).join(", ")} (set under ⚙)</>}
            </div>
            <div style={{ color: "#7e8ea8", fontSize: 10 }} title="once deployed">
              try: {s.tryIt}
            </div>
          </div>
          <button style={smallBtn} onClick={() => onOpenSample(s)} title={`open "${s.name}" as an unsaved copy`}>
            open
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
  onChanged,
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
  onChanged: () => void;
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
          onChanged={onChanged}
          result={triggerResults[entry.slug]}
        />
      ))}
    </div>
  );
}
