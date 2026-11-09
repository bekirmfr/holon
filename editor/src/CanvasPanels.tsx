// The editor's tab strip (Home, then one tab per open flow), its bar (the flow toolbar, above
// the canvas), and the projection panel (bottom right of the canvas).

import type { CSSProperties, ReactNode } from "react";
import type { WValue } from "./holon";
import { boundaryLabels, formatValue } from "./diagnostics";
import { BoundaryField } from "./BoundaryField";
import type { Projection } from "./canvasOps";
import type { Mode } from "./tabs";
import { Kbd } from "./ui/Kbd";

const primaryBtn: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  padding: "3px 10px",
  color: "var(--accent-ink)",
  background: "var(--accent)",
  border: "none",
  borderRadius: 3,
  cursor: "pointer",
};

const historyBtn = (enabled: boolean): CSSProperties => ({
  font: "inherit",
  fontSize: 11,
  padding: "3px 8px",
  color: "var(--muted)",
  background: "var(--surface-2)",
  border: "1px solid var(--border)",
  borderRadius: 3,
  cursor: enabled ? "pointer" : "default",
  opacity: enabled ? 1 : 0.4,
  whiteSpace: "nowrap",
});

const KIND_COLOR = { deployed: "var(--ok)", saved: "var(--muted)", draft: "var(--warn)" } as const;

/** The editor's tabs, as a browser's (gaps-web.md G16): Home first (a new tab's page), then a
 *  tab per open flow with what it is (draft, saved, deployed), its mode, ● while modified, and
 *  ×; + starts a draft in a tab of its own. */
export function FlowTabs({
  tabs,
  active,
  home,
  onHome,
  onSelect,
  onClose,
  onNew,
}: {
  tabs: { id: number; name: string | null; kind: "deployed" | "saved" | "draft"; dirty: boolean; mode: Mode }[];
  /** The active flow tab, or -1 while Home is shown. */
  active: number;
  home: boolean;
  onHome: () => void;
  onSelect: (id: number) => void;
  onClose: (id: number) => void;
  onNew: () => void;
}) {
  return (
    <div className="hl-tabs" role="tablist" aria-label="open tabs">
      <div role="tab" aria-selected={home} className={home ? "hl-tab hl-tab--active hl-tab--home" : "hl-tab hl-tab--home"} onClick={onHome} title="Home: open a flow, start one, the tabs closed lately, the keys">
        <span className="hl-tab__name">Home</span>
      </div>
      {tabs.map((t) => (
        <div
          key={t.id}
          role="tab"
          aria-selected={t.id === active}
          className={t.id === active ? "hl-tab hl-tab--active" : "hl-tab"}
          onClick={() => onSelect(t.id)}
          onAuxClick={(e) => e.button === 1 && onClose(t.id)}
          title={`${t.name ?? "untitled"}: ${t.kind}, ${t.mode === "view" ? "viewing its runs" : "edited"}${t.dirty ? ", modified" : ""}`}
        >
          <span className="hl-tab__name">{t.name ?? "untitled"}</span>
          {t.dirty && (
            <span className="hl-tab__dirty" aria-label="modified">
              ●
            </span>
          )}
          <span className="hl-tab__kind" style={{ color: KIND_COLOR[t.kind] }}>
            {t.kind}
          </span>
          {t.mode === "view" && <span className="hl-tab__mode">runs</span>}
          <button
            className="hl-tab__close"
            onClick={(e) => {
              e.stopPropagation();
              onClose(t.id);
            }}
            title="close this tab"
            aria-label={`close ${t.name ?? "untitled"}`}
          >
            ×
          </button>
        </div>
      ))}
      <button className="hl-tabs__new" onClick={onNew} title="a new draft, in a tab of its own" aria-label="new tab">
        +
      </button>
    </div>
  );
}

/** Edit or View runs (`e`): the same flow as edited (palette, save, deploy) or as deployed
 *  (read only, with its runs). Viewing needs a deployment. */
function ModeSwitch({ mode, canView, onMode }: { mode: Mode; canView: boolean; onMode: (m: Mode) => void }) {
  const seg = (m: Mode, label: string, enabled: boolean, title: string) => (
    <button
      onClick={() => enabled && m !== mode && onMode(m)}
      disabled={!enabled}
      title={title}
      aria-label={m}
      aria-pressed={m === mode}
      aria-keyshortcuts="e"
      style={{
        font: "inherit",
        fontSize: 11,
        padding: "3px 10px",
        border: "none",
        color: m === mode ? "var(--accent-ink)" : "var(--muted)",
        background: m === mode ? "var(--accent)" : "transparent",
        cursor: enabled && m !== mode ? "pointer" : "default",
        opacity: enabled ? 1 : 0.4,
      }}
    >
      {label}
    </button>
  );
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
      <span style={{ display: "inline-flex", border: "1px solid var(--border)", borderRadius: 3, overflow: "hidden" }}>
        {seg("edit", "Edit", true, "the flow as edited: place, wire, save and deploy (e)")}
        {seg("view", "View runs", canView, canView ? "the flow as deployed, read only, with its runs (e)" : "deploy it to view its runs")}
      </span>
      <Kbd k="e" />
    </span>
  );
}

/** A pane the bar shows or hides, by its key (gaps-web.md G17). Its button is named for the
 *  pane and whether it shows ("text on"). */
export type PaneToggle = { key: string; name: string; on: boolean; toggle: () => void };

/** The editor's bar, above the canvas: the flow's name and mode, the panes, then in edit mode
 *  save, undo/redo and the deploy bar (passed in as `deployBar`). `error` shows in both. */
export function FlowToolbar({
  histVersion,
  mode,
  canView,
  onMode,
  panes,
  flowName,
  dirty,
  onSave,
  canUndo,
  canRedo,
  onUndo,
  onRedo,
  deployBar,
  error,
}: {
  histVersion: number;
  mode: Mode;
  /** Whether it has a deployment to view. */
  canView: boolean;
  onMode: (m: Mode) => void;
  panes: PaneToggle[];
  flowName: string | null;
  dirty: boolean;
  onSave: () => void;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  deployBar: ReactNode;
  error: string;
}) {
  return (
    // histVersion is what makes the undo/redo disabled state live — history itself is a ref
    // (pushing must not re-render the canvas mid-drag), so this re-renders off the counter.
    <div data-hist={histVersion} style={{ display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center" }}>
      <span style={{ color: mode === "view" ? "var(--ink)" : "var(--muted)", fontSize: 12, letterSpacing: "0.04em" }}>
        {flowName ?? "untitled"}
        {dirty && <span title="modified: unsaved changes"> ●</span>}
      </span>
      <ModeSwitch mode={mode} canView={canView} onMode={onMode} />
      <span className="hl-panes" role="group" aria-label="panes">
        {panes.map((p) => (
          <button
            key={p.key}
            className="hl-panes__toggle"
            onClick={p.toggle}
            aria-pressed={p.on}
            aria-label={`${p.name} ${p.on ? "on" : "off"}`}
            aria-keyshortcuts={p.key}
            title={`${p.on ? "hide" : "show"} the ${p.name} (${p.key})`}
          >
            {p.name} <Kbd k={p.key} />
          </button>
        ))}
      </span>
      {mode === "view" ? (
        <span style={{ color: "var(--faint)", fontSize: 11 }}>deployed · read only</span>
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
        </>
      )}
      {error && <span style={{ color: "var(--bad)", fontSize: 11, maxWidth: 420 }}>{error}</span>}
    </div>
  );
}

const opBtn = (enabled: boolean): CSSProperties => ({
  font: "inherit",
  fontSize: 12,
  letterSpacing: "0.04em",
  padding: "5px 12px",
  color: "var(--accent-ink)",
  background: "var(--accent)",
  border: "none",
  borderRadius: 3,
  cursor: enabled ? "pointer" : "default",
  opacity: enabled ? 1 : 0.4,
});

/** Bottom right: project / group / ungroup, and the current projection (its hash, a field
 *  per promoted input, a result per output with a button that excludes it). */
export function ProjectionPanel({
  nodeCount,
  selectedCount,
  canExpand,
  onProject,
  onGroup,
  onExpand,
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
      {projection && (
        <div style={{ fontSize: 12, color: "var(--muted)", textAlign: "right", lineHeight: 1.4 }}>
          <div style={{ color: "var(--accent)" }} title={projection.hash}>
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
            <div style={{ display: "flex", flexDirection: "column", gap: 2, alignItems: "flex-end", color: "var(--ok)" }}>
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
                      color: "var(--muted)",
                      background: "var(--surface-2)",
                      border: "1px solid var(--border)",
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
      {error && <div style={{ fontSize: 12, color: "var(--bad)", maxWidth: 240, textAlign: "right" }}>{error}</div>}
    </div>
  );
}
