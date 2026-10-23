// The canvas's tab strip (one tab per open flow), and its two button panels: the flow toolbar
// (top left) and the projection panel (top right).

import type { CSSProperties, ReactNode } from "react";
import type { WValue } from "./holon";
import { boundaryLabels, formatValue } from "./diagnostics";
import { BoundaryField } from "./BoundaryField";
import type { Projection } from "./canvasOps";
import type { Mode } from "./tabs";

const primaryBtn: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  padding: "3px 10px",
  color: "#0e1a2f",
  background: "#6fd3e0",
  border: "none",
  borderRadius: 3,
  cursor: "pointer",
};

const historyBtn = (enabled: boolean): CSSProperties => ({
  font: "inherit",
  fontSize: 11,
  padding: "3px 8px",
  color: "#9fb4d4",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  cursor: enabled ? "pointer" : "default",
  opacity: enabled ? 1 : 0.4,
});

const KIND_COLOR = { deployed: "#9ad29f", saved: "#9fb4d4", draft: "#e0c36f" } as const;

/** Above the canvas: a tab per open flow, with what it is (draft, saved, deployed), its mode,
 *  unsaved changes, and a close button. */
export function FlowTabs({
  tabs,
  active,
  onSelect,
  onClose,
  pages,
}: {
  tabs: { id: number; name: string | null; kind: "deployed" | "saved" | "draft"; dirty: boolean; mode: Mode }[];
  /** The active flow tab, or -1 while a page tab (home, welcome) is shown. */
  active: number;
  onSelect: (id: number) => void;
  onClose: (id: number) => void;
  /** The tabs before the flows: home (pinned), and the welcome while it's open. */
  pages: { key: string; label: string; active: boolean; onSelect: () => void; onClose?: () => void }[];
}) {
  return (
    <div className="hl-tabs" role="tablist">
      {pages.map((p) => (
        <div
          key={p.key}
          role="tab"
          aria-selected={p.active}
          className={p.active ? "hl-tab hl-tab--active" : "hl-tab"}
          onClick={p.onSelect}
          title={p.label}
        >
          <span className="hl-tab__name">{p.label}</span>
          {p.onClose && (
            <button
              className="hl-tab__close"
              onClick={(e) => {
                e.stopPropagation();
                p.onClose!();
              }}
              title={`close the ${p.label}`}
            >
              ×
            </button>
          )}
        </div>
      ))}
      {tabs.map((t) => (
        <div
          key={t.id}
          role="tab"
          aria-selected={t.id === active}
          className={t.id === active ? "hl-tab hl-tab--active" : "hl-tab"}
          onClick={() => onSelect(t.id)}
          onAuxClick={(e) => e.button === 1 && onClose(t.id)}
          title={`${t.name ?? "untitled"}: ${t.kind}, ${t.mode} mode${t.dirty ? ", unsaved changes" : ""}`}
        >
          <span className="hl-tab__name">{t.name ?? "untitled"}</span>
          {t.dirty && <span className="hl-tab__dirty">•</span>}
          <span className="hl-tab__kind" style={{ color: KIND_COLOR[t.kind] }}>
            {t.kind}
          </span>
          {t.mode === "view" && <span className="hl-tab__mode">view</span>}
          <button
            className="hl-tab__close"
            onClick={(e) => {
              e.stopPropagation();
              onClose(t.id);
            }}
            title="close this tab"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

/** Edit or view: the same flow as edited (palette, save, deploy) or as deployed (read only,
 *  with its runs). View needs a deployment. */
function ModeSwitch({ mode, canView, onMode }: { mode: Mode; canView: boolean; onMode: (m: Mode) => void }) {
  const seg = (m: Mode, enabled: boolean, title: string) => (
    <button
      onClick={() => enabled && m !== mode && onMode(m)}
      disabled={!enabled}
      title={title}
      style={{
        font: "inherit",
        fontSize: 11,
        padding: "3px 10px",
        border: "none",
        color: m === mode ? "#0e1a2f" : "#9fb4d4",
        background: m === mode ? "#6fd3e0" : "transparent",
        cursor: enabled && m !== mode ? "pointer" : "default",
        opacity: enabled ? 1 : 0.4,
      }}
    >
      {m}
    </button>
  );
  return (
    <span style={{ display: "inline-flex", border: "1px solid #1b2a45", borderRadius: 3, overflow: "hidden" }}>
      {seg("edit", true, "the flow as edited: place, wire, save and deploy")}
      {seg("view", canView, canView ? "the flow as deployed, read only, with its runs" : "deploy it to view its runs")}
    </span>
  );
}

/** Top left: the flow's name and mode, then in edit mode save, undo/redo and the deploy bar
 *  (passed in as `deployBar`), in view mode the values toggle. `error` shows in both. */
export function FlowToolbar({
  histVersion,
  mode,
  canView,
  onMode,
  showValues,
  onToggleValues,
  flowName,
  dirty,
  onSave,
  canUndo,
  canRedo,
  onUndo,
  onRedo,
  deployBar,
  saveMsg,
  error,
}: {
  histVersion: number;
  mode: Mode;
  /** Whether it has a deployment to view. */
  canView: boolean;
  onMode: (m: Mode) => void;
  /** View mode: whether nodes show their outputs' values as chips. */
  showValues: boolean;
  onToggleValues: () => void;
  flowName: string | null;
  dirty: boolean;
  onSave: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  deployBar: ReactNode;
  saveMsg: string;
  error: string;
}) {
  return (
    // histVersion is what makes the undo/redo disabled state live — history itself is a ref
    // (pushing must not re-render the canvas mid-drag), so this re-renders off the counter.
    <div data-hist={histVersion} style={{ display: "flex", gap: 6, alignItems: "center" }}>
      <span style={{ color: mode === "view" ? "#e6eefb" : "#9fb4d4", fontSize: 12, letterSpacing: "0.04em" }}>
        {flowName ?? "untitled"}
        {dirty && <span title="unsaved changes"> •</span>}
      </span>
      <ModeSwitch mode={mode} canView={canView} onMode={onMode} />
      {mode === "view" ? (
        <>
          <span style={{ color: "#7e8ea8", fontSize: 11 }}>deployed · read only</span>
          <button
            onClick={onToggleValues}
            style={historyBtn(true)}
            title={showValues ? "hide the values: the flow exactly as designed" : "show each output's value beside it"}
          >
            values {showValues ? "on" : "off"}
          </button>
        </>
      ) : (
        <>
          <button onClick={onSave} style={primaryBtn}>
            save
          </button>
          <button onClick={onUndo} disabled={!canUndo} title="undo (Ctrl+Z)" style={historyBtn(canUndo)}>
            ⟲
          </button>
          <button onClick={onRedo} disabled={!canRedo} title="redo (Ctrl+Y / Ctrl+Shift+Z)" style={historyBtn(canRedo)}>
            ⟳
          </button>
          {deployBar}
          {saveMsg && <span style={{ color: "#9ad29f", fontSize: 11 }}>{saveMsg}</span>}
        </>
      )}
      {error && <span style={{ color: "#e0916f", fontSize: 11, maxWidth: 420 }}>{error}</span>}
    </div>
  );
}

const opBtn = (enabled: boolean): CSSProperties => ({
  font: "inherit",
  fontSize: 12,
  letterSpacing: "0.04em",
  padding: "5px 12px",
  color: "#0e1a2f",
  background: "#6fd3e0",
  border: "none",
  borderRadius: 3,
  cursor: enabled ? "pointer" : "default",
  opacity: enabled ? 1 : 0.4,
});

/** Top right: project / group / ungroup / inspect, and the current projection (its hash, a field
 *  per promoted input, a result per output with a button that excludes it). */
export function ProjectionPanel({
  nodeCount,
  selectedCount,
  canExpand,
  onProject,
  onGroup,
  onExpand,
  onInspect,
  projection,
  onBoundaryChange,
  onExcludeOutput,
  error,
}: {
  nodeCount: number;
  selectedCount: number;
  canExpand: boolean;
  onProject: () => void;
  onGroup: () => void;
  onExpand: () => void;
  onInspect: () => void;
  projection: Projection | null;
  onBoundaryChange: (index: number, value: WValue | null) => void;
  onExcludeOutput: (index: number) => void;
  error: string;
}) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-end" }}>
      <button onClick={onProject} disabled={nodeCount === 0} style={opBtn(nodeCount > 0)}>
        project
      </button>
      <button
        onClick={onGroup}
        disabled={selectedCount < 2}
        title={selectedCount < 2 ? "select 2+ nodes to group" : `group ${selectedCount} nodes`}
        style={opBtn(selectedCount >= 2)}
      >
        group
      </button>
      <button
        onClick={onExpand}
        disabled={!canExpand}
        title={canExpand ? "ungroup: replace this composite with the nodes inside it" : "select one composite node to ungroup (to look inside it, select it and expand)"}
        style={opBtn(canExpand)}
      >
        ungroup
      </button>
      <button
        onClick={onInspect}
        disabled={selectedCount !== 1}
        title={selectedCount === 1 ? "inspect this node" : "select one node to inspect"}
        style={opBtn(selectedCount === 1)}
      >
        inspect
      </button>
      {projection && (
        <div style={{ fontSize: 12, color: "#9fb4d4", textAlign: "right", lineHeight: 1.4 }}>
          <div style={{ color: "#6fd3e0" }} title={projection.hash}>
            {projection.hash.slice(0, 12)}…
          </div>
          {projection.ports.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 4, margin: "6px 0", alignItems: "flex-end" }}>
              {boundaryLabels(projection.ports, projection.source).map((label, i) => (
                <BoundaryField
                  key={i}
                  port={projection.ports[i]}
                  label={label}
                  value={projection.values[i]}
                  onChange={(v) => onBoundaryChange(i, v)}
                />
              ))}
            </div>
          )}
          {projection.results.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 2, alignItems: "flex-end", color: "#9ad29f" }}>
              {boundaryLabels(projection.outputs, projection.resultSource).map((label, i) => (
                <div key={i} style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  <span>
                    {label} = {projection.results[i] ? formatValue(projection.results[i]!) : "not ready"}
                  </span>
                  <button
                    className="nodrag"
                    onClick={() => onExcludeOutput(i)}
                    title={`exclude "${label}" from this composite's interface`}
                    style={{
                      font: "inherit",
                      fontSize: 10,
                      lineHeight: 1,
                      color: "#9fb4d4",
                      background: "#0e1a2f",
                      border: "1px solid #1b2a45",
                      borderRadius: 3,
                      width: 14,
                      height: 14,
                      padding: 0,
                      cursor: "pointer",
                    }}
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
      {error && <div style={{ fontSize: 12, color: "#e0916f", maxWidth: 240, textAlign: "right" }}>{error}</div>}
    </div>
  );
}
