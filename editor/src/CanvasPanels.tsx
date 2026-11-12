// The editor's tab strip (Home, then one tab per open flow), its bar (the flow toolbar, above
// the canvas: web-console-L4.md §5.1), and the projection panel (bottom right of the canvas).

import type { CSSProperties } from "react";
import type { WValue } from "./holon";
import { boundaryLabels, formatValue } from "./diagnostics";
import { BoundaryField } from "./BoundaryField";
import type { Projection } from "./canvasOps";
import type { Mode } from "./tabs";
import type { CanvasLevel } from "./canvasLevel";
import { Kbd } from "./ui/Kbd";
import { gateOf, type Answer } from "./useCan";
import type { DeploymentEntry } from "./server";
import { follow, format, type Route } from "./routes";

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

const STATUS_MARK: Record<DeploymentEntry["status"], string> = { running: "●", paused: "❚❚", stopped: "■" };

/** What the bar says of the file: `v3 live` while a version of it is deployed (`v3 paused`,
 *  `v3 stopped`), else `saved` or `draft`; and `changed` while it has unsaved changes. */
export function fileState(entry: DeploymentEntry | undefined, saved: boolean, dirty: boolean): string[] {
  const deployed = entry ? `${entry.version ? `v${entry.version} ` : ""}${entry.status === "running" ? "live" : entry.status}` : null;
  return [deployed ?? (saved ? "saved" : "draft"), ...(dirty ? ["changed"] : [])];
}

/** The editor's bar, above the canvas (web-console-L4.md §5.1): the mode, the file with its state,
 *  the panes; in Edit, format, check, save and deploy; in View runs, its runs and history on its
 *  page. A deployed flow's status is a chip that goes to its page, where it is paused, resumed,
 *  stopped and run. `error` shows in both. The canvas's own controls are its head's (CanvasHead). */
export function FlowToolbar({
  mode,
  canView,
  onMode,
  panes,
  flowName,
  saved,
  dirty,
  entry,
  navigate,
  edit,
  error,
}: {
  mode: Mode;
  /** Whether it has a deployment to view. */
  canView: boolean;
  onMode: (m: Mode) => void;
  panes: PaneToggle[];
  flowName: string | null;
  /** Whether it is saved on the server, and whether it has unsaved changes. */
  saved: boolean;
  dirty: boolean;
  /** Its deployment, if it has one. */
  entry: DeploymentEntry | undefined;
  navigate: (r: Route) => void;
  /** Edit's buttons: format and check the text, save it (`maySave`: `flow.edit`, or why not),
   *  and deploy it (`mayDeploy`: `flow.propose`, as `POST /deploy` asks; an agent's version that
   *  needs a person waits as a proposal). `outOfDate`: the flow no longer matches what is
   *  deployed; `startsNothing`: nothing would start a run, so it doesn't deploy. */
  edit: {
    onFormat: () => void;
    onCheck: () => void;
    onSave: () => void;
    maySave?: Answer;
    onDeploy: () => void;
    mayDeploy?: Answer;
    busy: boolean;
    outOfDate: boolean | null;
    startsNothing: boolean;
  };
  error: string;
}) {
  const link = (to: Route, label: string, title: string) => (
    <a className="hl-bar__link" href={format(to)} onClick={follow(to, navigate)} title={title}>
      {label}
    </a>
  );
  const slug = flowName ?? "";
  const nothing =
    "nothing would start a run: add a route, a websocket, a clock, a file, mail, feed or queue handler or an action (an action runs by hand)";
  const deployOff = edit.busy || edit.startsNothing || (edit.mayDeploy ?? true) !== true;
  return (
    <div className="hl-bar">
      <ModeSwitch mode={mode} canView={canView} onMode={onMode} />
      <span className="hl-bar__file" title={dirty ? "unsaved changes" : undefined}>
        <span className="hl-bar__name">{flowName ?? "untitled"}.holon.ts</span>
        {fileState(entry, saved, dirty).map((s) => (
          <span key={s} className={s === "changed" ? "hl-bar__state hl-bar__state--changed" : "hl-bar__state"}>
            {s}
          </span>
        ))}
      </span>
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
        <>
          {link({ kind: "flow", slug, tab: "runs" }, "All runs", "every run of it, on its page")}
          {link({ kind: "flow", slug, tab: "history" }, "History", "its versions and what changed, on its page")}
        </>
      ) : (
        <>
          <button className="hl-bar__button" onClick={edit.onFormat} title="as holon fmt writes it (Shift+Alt+F)">
            format
          </button>
          <button className="hl-bar__button" onClick={edit.onCheck} title="look for mistakes now, as holon check does: listed in Problems">
            check
          </button>
          <button onClick={edit.onSave} style={primaryBtn} {...gateOf(edit.maySave ?? true)}>
            save
          </button>
          <button
            onClick={edit.onDeploy}
            disabled={deployOff}
            title={edit.mayDeploy !== undefined && edit.mayDeploy !== true ? edit.mayDeploy : edit.startsNothing ? nothing : entry ? "deploy the flow as it is now; the deployment keeps its state and sessions" : "deploy this flow, named after it"}
            style={{ ...primaryBtn, opacity: deployOff ? 0.5 : 1, cursor: deployOff ? "default" : "pointer" }}
          >
            {edit.busy ? "deploying…" : entry ? "update" : "deploy"}
            {edit.outOfDate && <span title="the canvas has changed since it was deployed"> •</span>}
          </button>
          {edit.startsNothing && (
            <span style={{ color: "var(--warn)", fontSize: 11 }} title={nothing}>
              nothing would start a run
            </span>
          )}
        </>
      )}
      {entry && (
        <a
          className={`hl-bar__status hl-flow__status--${entry.status}`}
          href={format({ kind: "flow", slug, tab: "status" })}
          onClick={follow({ kind: "flow", slug, tab: "status" }, navigate)}
          title={`its page: pause, resume, stop or run it (session ${entry.session})`}
        >
          {STATUS_MARK[entry.status]} {entry.status}
          {entry.port !== null && entry.status !== "stopped" ? ` :${entry.port}` : ""}
        </a>
      )}
      {entry?.attention && (
        <span style={{ color: "var(--bad)", fontSize: 11 }} title={entry.attention}>
          ⚠ needs approval
        </span>
      )}
      {entry?.listen_error && (
        <span style={{ color: "var(--bad)", fontSize: 11 }} title={entry.listen_error}>
          ⚠ port in use
        </span>
      )}
      {error && <span style={{ color: "var(--bad)", fontSize: 11, maxWidth: 420 }}>{error}</span>}
    </div>
  );
}

/** The canvas pane's head: its level, Statements or Nodes (`l`), and at the Nodes level the
 *  canvas's own controls: fit, and while it is edited undo, redo and tidy. */
export function CanvasHead({
  level,
  onLevel,
  editing,
  histVersion,
  canUndo,
  canRedo,
  onUndo,
  onRedo,
  onTidy,
  onFit,
}: {
  level: CanvasLevel;
  onLevel: (l: CanvasLevel) => void;
  /** In edit mode: undo, redo and tidy are offered. */
  editing: boolean;
  histVersion: number;
  canUndo: boolean;
  canRedo: boolean;
  onUndo: () => void;
  onRedo: () => void;
  /** Lays the flow out again (the shared tidy layout, graphLayout.ts): the nodes only. */
  onTidy: () => void;
  /** Fits the whole flow in the view. */
  onFit: () => void;
}) {
  const seg = (l: CanvasLevel, label: string, title: string) => (
    <button
      className="hl-levels__seg"
      onClick={() => l !== level && onLevel(l)}
      aria-label={label}
      aria-pressed={l === level}
      aria-keyshortcuts="l"
      title={`${title} (l)`}
    >
      {label}
    </button>
  );
  return (
    // histVersion is what makes the undo/redo disabled state live — history itself is a ref
    // (pushing must not re-render the canvas mid-drag), so this re-renders off the counter.
    <div className="hl-canvas__head" data-hist={histVersion}>
      <span className="hl-canvas__title">Canvas</span>
      <kbd className="hl-pane-key" aria-hidden="true">
        c
      </kbd>
      <span className="hl-levels" role="group" aria-label="canvas level">
        {seg("statements", "Statements", "the flow by its statements, as the text says it: read only, edited through the text")}
        {seg("nodes", "Nodes", "the nodes the flow runs: place, wire, group")}
      </span>
      <Kbd k="l" />
      <span style={{ flex: 1 }} />
      {level === "nodes" && (
        <span className="hl-canvas__controls">
          <button onClick={onFit} title="fit: the whole flow in the view" style={historyBtn(true)}>
            ⤢
          </button>
          {editing && (
            <>
              <button onClick={onUndo} disabled={!canUndo} title="undo (Ctrl+Z)" style={historyBtn(canUndo)}>
                ⟲
              </button>
              <button onClick={onRedo} disabled={!canRedo} title="redo (Ctrl+Y / Ctrl+Shift+Z)" style={historyBtn(canRedo)}>
                ⟳
              </button>
              <button onClick={onTidy} title="tidy: lay the flow out by dependency, a domain per handler" style={historyBtn(true)}>
                ▤
              </button>
            </>
          )}
        </span>
      )}
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
