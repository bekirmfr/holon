// The canvas's two button panels: the flow toolbar (top left) and the projection panel (top right).

import type { CSSProperties, ReactNode } from "react";
import type { WValue } from "./holon";
import { boundaryLabels, formatValue } from "./diagnostics";
import { BoundaryField } from "./BoundaryField";
import type { Projection } from "./canvasOps";

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

/** Top left: the deployed flow being viewed and the way out, or the flow being edited with
 *  save, undo/redo and the deploy bar (passed in as `deployBar`). `error` shows in both. */
export function FlowToolbar({
  histVersion,
  viewing,
  onExitView,
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
  viewing: string | null;
  onExitView: () => void;
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
      {viewing ? (
        <>
          <span style={{ color: "#e6eefb", fontSize: 12, letterSpacing: "0.04em" }}>
            viewing <span style={{ color: "#6fd3e0" }}>{viewing}</span>
          </span>
          <span style={{ color: "#7e8ea8", fontSize: 11 }}>deployed · read only</span>
          <button onClick={onExitView} style={primaryBtn}>
            exit view
          </button>
        </>
      ) : (
        <>
          <span style={{ color: "#9fb4d4", fontSize: 12, letterSpacing: "0.04em" }}>
            {flowName ?? "untitled"}
            {dirty && <span title="unsaved changes"> •</span>}
          </span>
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

/** Top right: project / group / expand / inspect, and the current projection (its hash, a field
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
        title={canExpand ? "expand this composite" : "select one composite node to expand"}
        style={opBtn(canExpand)}
      >
        expand
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
