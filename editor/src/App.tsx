// The authoring canvas: a palette (derived from core's catalog) on the left, a blueprint canvas on
// the right. Drag a primitive across to place a typed node. This inch places nodes only — wiring and
// projection/eval are later inches (P4.1-palette-L4 §1). Nothing here re-asserts a primitive's shape;
// every placed node reads its ports from the catalog interface (I1).

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  Controls,
  Panel,
  useNodesState,
  useEdgesState,
  useReactFlow,
  addEdge,
  type NodeTypes,
  type Connection,
  type Edge,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import {
  ensureReady,
  primitiveCatalog,
  variadicPrimitives,
  compatible,
  buildPreimage,
  Evaluator,
  BOUNDARY,
  type WUnit,
  type WEdge,
  type WValue,
  type WPort,
  type WType,
  type WEncapSpec,
  type WCapability,
} from "./holon";
import {
  stageUnit,
  deployUnit,
  listDeployments,
  deleteDeployment,
  changeDeployment,
  triggerDeployment,
  saveFlow,
  listFlows,
  getFlow,
  deleteFlow,
  ServerError,
  type DeploymentEntry,
  type TriggerResult,
  type FlowDocument,
  type FlowListEntry,
  type FlowRecord,
} from "./server";
import { outputColor, typeColor, typeLabel } from "./theme";
import { Palette, DND_MIME } from "./Palette";
import { PrimitiveNode, type PrimitiveData, type PrimitiveNodeType } from "./PrimitiveNode";
import { InspectPanel } from "./InspectPanel";
import { closureOf, documentKey, fromHex, structuralKey, toHex } from "./flowDoc";
import { History } from "./history";
import { DialogProvider, useDialog } from "./Dialog";
import { DeploymentCard } from "./DeploymentCard";
import { ServerPanel } from "./ServerPanel";
import { DeployBar, DEFAULT_SETTINGS, type FlowSettings } from "./DeployBar";
import { buildTemplate, isTemplate, templateDefaults, templateInterface, TEMPLATES, type TemplateName } from "./templates";
import { findRoutes, routeSpec } from "./derive";
import { endpointUnit, serverUnit, type EndpointDraft } from "./endpoint";

// What a save records, for unsaved-change tracking: the canvas plus the server draft.
const savedKeyOf = (ns: PrimitiveNodeType[], es: Edge[], settings: FlowSettings) =>
  documentKey(ns, es) + JSON.stringify(settings);
import {
  boundaryLabels,
  isWEncapError,
  encapErrorNodeIndices,
  capabilityAcknowledgeName,
  formatValue,
} from "./diagnostics";

const nodeTypes = { primitive: PrimitiveNode } as unknown as NodeTypes;

function primitiveName(u: WUnit): string | null {
  return u.body.kind === "Primitive" ? u.body.name : null;
}

// Render an evaluated value for the readout. This is core's *pure* evaluator, so a sink shows as the
// Effect it constructs (it doesn't fire here), and a driver / unparsed-const graph comes back null.
// formatValue moved to ./diagnostics (this inch) — pure, no wasm dependency, and worth actually
// testing after the Effect case shipped a real, silent bug (see its own doc there).

// A promoted input's starting value: the type's zero value for the three scalar forms that have
// one (Number→0, Boolean→false, Text→"") — the SAME convention `const`'s own default and core's
// `sourced_mock` already use, reused rather than invented fresh. Everything else (List/Product/Sum/
// Effect/Any) has no literal form — same boundary `const`'s CONST_TYPES already draws — so it starts
// `null` ("wire in": honestly unsupplied, not a guessed default).
//
// A `null` slot does NOT mean "eval blocks" — core's `eval` drops absent slots before `compute` ever
// runs (`inputs.iter().flatten()`; "compute's arity check is the firing rule" per eval.rs's own
// comment), uniformly for every primitive, no special-casing here or there. Whether that reads as
// "not ready" depends entirely on the target primitive: `add` destructures exactly 2 values, so a
// dropped slot fails its own arity match ⇒ not ready. `list` has no arity requirement at all, so it
// happily builds from however many values are actually present — including zero, `[]`. Both are
// correct; neither needs different handling in this file.
function defaultBoundaryValue(ty: WType): WValue | null {
  if (ty === "Number") return { type: "Number", value: 0 };
  if (ty === "Boolean") return { type: "Boolean", value: false };
  if (ty === "Text") return { type: "Text", value: "" };
  return null;
}

const boundaryFieldStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#cfe3ff",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 4px",
  width: 64,
};

// One promoted input's field. A scalar gets a real, type-native control — a number input, a
// checkbox, a text input — not a generic text box needing string parsing: unlike `const`, a
// boundary value is never hashed (it's a pure eval-time argument), so there's no reason to route
// it through core's own string parser the way a frozen param has to. A non-scalar honestly has no
// literal form (the same boundary CONST_TYPES already draws) and shows "wire in", not a guess.
// Colour comes from theme.ts — the same "type is colour" legend the canvas nodes already use.

// boundaryLabels / isWEncapError / validationErrorNodeIndices / encapErrorNodeIndices moved to
// ./diagnostics (2026-07-02) — they need nothing from this file, and extracting them keeps them
// testable without pulling in this file's own top-level value import of Evaluator from ./holon.

// A placed node's own hash, for use as a child in project/group (P4.3.1). A genesis primitive
// is rebuilt fresh from its catalog name+interface — I3 (the same primitive always yields the
// same hash) makes "rebuild" and "reference" the same operation, exactly how onProject has
// always worked. A grouped composite is different: it was already frozen when it was grouped,
// so its hash is just read off PlacedBody directly — its own internal structure is neither
// rebuilt nor re-inspected here. Shared by onProject and onGroup so there's one implementation
// of "how do I get this node's hash," not two that could drift.
function unitFor(n: PrimitiveNodeType, ev: Evaluator, catalog: Map<string, WUnit>): string {
  if (n.data.body.kind === "Composite") return n.data.body.hash;
  if (n.data.body.kind === "Template") {
    const u = buildTemplate(n.data.name as TemplateName, n.data.params, catalog, (c) => ev.insert(buildPreimage(c)));
    if (typeof u === "string") throw new Error(`${n.data.name}: ${u}`);
    return ev.insert(buildPreimage(u));
  }
  const wunit: WUnit = {
    body: { kind: "Primitive", name: n.data.name },
    params: n.data.params,
    inputs: n.data.inputs,
    outputs: n.data.outputs,
  };
  return ev.insert(buildPreimage(wunit));
}

// Recursively stage a unit's whole closure to holon-server — not just the root. A composite's
// own preimage references its children BY HASH; it never carries their bytes. The local
// Evaluator already has every one of them (that's how the composite got built in the first
// place), so this walks the SAME closure `capabilities()`/`materialize_closure` walk on the
// server side — children, and any `fn` param (a `map`'s higher-order reference; core's own
// `child_refs` counts it as part of the closure too, so skipping it here would under-stage
// exactly the case that exists to not miss) — and pushes whatever the server doesn't have yet.
// `staged` is the caller's own de-dup set: the same child reachable from two different parents
// (an ordinary DAG, not a tree) only needs sending once. Order between siblings doesn't matter —
// `POST /unit` stores whatever bytes it's given with no closure-completeness check of its own
// (that's `/deploy`'s job, downstream) — only "every hash gets there before `/deploy` is called"
// matters, not the order they arrive in.
async function stageClosure(ev: Evaluator, hash: string, staged: Set<string>): Promise<void> {
  if (staged.has(hash)) return;
  staged.add(hash); // mark before awaiting anything, so a re-entrant call can't double-queue it
  const unit = ev.get_unit(hash) as WUnit;
  if (unit.body.kind === "Composite") {
    for (const child of unit.body.children) {
      await stageClosure(ev, child, staged);
    }
  }
  const fnParam = (unit.params ?? []).find((p) => p.kind === "Frozen" && p.name === "fn");
  if (fnParam && fnParam.kind === "Frozen") {
    await stageClosure(ev, fnParam.value, staged);
  }
  await stageUnit(buildPreimage(unit));
}

function BoundaryField({
  port,
  label: displayLabel,
  value,
  onChange,
}: {
  port: WPort;
  label: string;
  value: WValue | null;
  onChange: (v: WValue | null) => void;
}) {
  const row: CSSProperties = { display: "flex", alignItems: "center", justifyContent: "flex-end", gap: 6 };
  const label = (
    <span style={{ color: typeColor(port.ty), fontSize: 11 }} title={typeLabel(port.ty)}>
      {displayLabel}
    </span>
  );

  if (port.ty === "Number") {
    const raw = value?.type === "Number" ? String(value.value) : "";
    return (
      <div style={row}>
        {label}
        <input
          type="number"
          style={boundaryFieldStyle}
          value={raw}
          onChange={(e) => {
            const v = e.target.value;
            onChange(v === "" ? null : { type: "Number", value: Number(v) });
          }}
        />
      </div>
    );
  }
  if (port.ty === "Boolean") {
    const checked = value?.type === "Boolean" ? value.value : false;
    return (
      <div style={row}>
        {label}
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange({ type: "Boolean", value: e.target.checked })}
        />
      </div>
    );
  }
  if (port.ty === "Text") {
    const raw = value?.type === "Text" ? value.value : "";
    return (
      <div style={row}>
        {label}
        <input
          type="text"
          style={{ ...boundaryFieldStyle, width: 84 }}
          value={raw}
          onChange={(e) => onChange({ type: "Text", value: e.target.value })}
        />
      </div>
    );
  }
  // List / Product / Sum / Effect / Any — no literal form. Honestly unsupplied, not defaulted.
  return (
    <div style={row}>
      {label}
      <span style={{ ...boundaryFieldStyle, color: "#5a6d8c", textAlign: "center" }}>wire in</span>
    </div>
  );
}

/** Which top-level view is showing. The canvas is never unmounted (React Flow's own state —
 *  nodes mid-edit, viewport — lives there and must survive navigation); the other pages render
 *  as overlays on top of it. `marketplace` is deliberately absent: deferred, and a menu entry
 *  that can't be picked shouldn't be representable as a page state either. */
type AppPage = "canvas" | "flows" | "deployed";

/** The saved-flows page: list, open, delete. Publishing to the marketplace is deferred — the
 *  button is real but disabled, honestly labeled, rather than wired to something half-built. */
function FlowsPage({ onOpen }: { onOpen: (rec: FlowRecord) => void }) {
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
          <button style={smallBtn} onClick={() => open(f.name)}>
            open
          </button>
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

function Canvas({
  catalog,
  variadic,
  page,
  setPage,
  pendingNew,
  onDirtyChange,
}: {
  catalog: WUnit[];
  variadic: Set<string>;
  page: AppPage;
  setPage: (p: AppPage) => void;
  pendingNew: { name: string; nonce: number } | null;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const dialog = useDialog();
  const [nodes, setNodes, onNodesChange] = useNodesState<PrimitiveNodeType>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { screenToFlowPosition, getNode, getEdges, getViewport, setViewport } =
    useReactFlow<PrimitiveNodeType>();
  const idRef = useRef(0);
  // The one in-browser workspace every placed composite lives in (see Projection below).
  // Declared first because saving and opening a flow read and fill it too.
  const ev = useMemo(() => new Evaluator(), []);
  // Projection state, declared here (not with the Projection section below) because New and
  // Open reset it. See that section for what each field means.
  const [projection, setProjection] = useState<{
    hash: string;
    pre: Uint8Array;
    ports: WPort[];
    source: (string | null)[];
    values: (WValue | null)[];
    outputs: WPort[];
    resultSource: (string | null)[];
    results: (WValue | null)[];
    // The exact inputs this projection was built from, held so excludeOutput below can
    // re-freeze without re-deriving from the (possibly since-changed) canvas — the same
    // "frozen at project time" reasoning `pre` already follows for boundary re-eval.
    children: string[];
    wedges: WEdge[];
    spec: WEncapSpec;
    // The canvas's structuralKey when this was projected. Once the canvas no longer matches,
    // the projection is dropped (below) rather than left on screen, deployable.
    basis: string;
  } | null>(null);
  const [projError, setProjError] = useState<string>("");
  // Which placed nodes a projection error implicates — derived from the structured error's own
  // indices (see encapErrorNodeIndices), not guessed. Purely presentational: never read by
  // onProject/onDrop, never affects hashing — a node's identity doesn't know it's "wrong."
  const [errorNodeIds, setErrorNodeIds] = useState<Set<string>>(new Set());
  // Which hash is currently shown in the inspect panel (P4.4.1) — null when closed. Purely a
  // view concern, like errorNodeIds above: never read by onProject/onGroup/onExpand, never
  // affects the canvas or hashing. Read-only, deliberately unlike expand (P4.3.1): opening this
  // never touches `nodes`/`edges` at all.
  const [inspecting, setInspecting] = useState<string | null>(null);

  const canvasKey = useMemo(() => structuralKey(nodes, edges), [nodes, edges]);
  useEffect(() => {
    setProjection((p) => (p && p.basis !== canvasKey ? null : p));
  }, [canvasKey]);

  // New and Open start from a clean slate: no projection, error highlight or inspect panel
  // carried over from the previous flow.
  const resetProjectionView = useCallback(() => {
    setProjection(null);
    setProjError("");
    setErrorNodeIds(new Set());
    setInspecting(null);
    setDeployError(""); // declared with the deploy state below; only called after render
  }, []);

  // ── the current flow (P4.8): its name, save, and undo/redo ────────────────────────────
  // A saved flow is a named workspace document — nodes/edges/viewport exactly as left, stored
  // opaquely by the server (see server.ts's FlowDocument). Deliberately separate from the
  // content-addressed unit the same canvas projects to: positions and zoom are presentation,
  // and presentation stays out of identity by design.
  const [flowName, setFlowName] = useState<string | null>(null);
  const [saveMsg, setSaveMsg] = useState("");

  // Unsaved changes: the canvas differs from what was last saved, opened, or started fresh.
  // New and Open ask before replacing it, and closing the tab asks too.
  // The flow's deploy settings (DeployBar's ⚙): saved and opened with the flow.
  const [settings, setSettings] = useState<FlowSettings>(DEFAULT_SETTINGS);
  const [savedKey, setSavedKey] = useState(() => savedKeyOf([], [], DEFAULT_SETTINGS));
  const dirty = useMemo(
    () => savedKeyOf(nodes, edges, settings) !== savedKey,
    [nodes, edges, settings, savedKey],
  );
  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  // Undo/redo: a debounced snapshot history over {nodes, edges}. React Flow has no built-in
  // history, so this is the standard minimal shape: commit a snapshot 400ms after the canvas
  // settles (so a drag is ONE entry, not sixty), undo/redo swap between past/committed/future.
  // Selection and drag flags are normalized out of snapshots — undoing a *selection* would be
  // noise, and restoring a node mid-"dragging" would wedge it to the cursor. `restoring` marks
  // programmatic setNodes/setEdges (undo, redo, open, new) so the watcher records them as the
  // new baseline instead of pushing them into history as if the user had made an edit. The
  // bookkeeping itself lives in ./history (History), which undo/redo hand the live canvas so an
  // edit still inside the debounce window is committed first rather than lost.
  type Snapshot = { nodes: PrimitiveNodeType[]; edges: Edge[] };
  const history = useRef(
    new History<Snapshot>({ nodes: [], edges: [] }, (a, b) => JSON.stringify(a) === JSON.stringify(b)),
  );
  const restoring = useRef(false);
  const [histVersion, setHistVersion] = useState(0); // buttons re-render on history changes
  const live = useRef({ nodes, edges });
  live.current = { nodes, edges };

  const snapshotOf = useCallback((ns: PrimitiveNodeType[], es: Edge[]): Snapshot => {
    return {
      nodes: ns.map((n) => ({ ...n, selected: false, dragging: false })),
      edges: es.map((e) => ({ ...e, selected: false })),
    };
  }, []);

  useEffect(() => {
    if (restoring.current) {
      restoring.current = false;
      history.current.committed = snapshotOf(nodes, edges);
      return;
    }
    const t = setTimeout(() => {
      if (history.current.commit(snapshotOf(nodes, edges))) setHistVersion((v) => v + 1);
    }, 400);
    return () => clearTimeout(t);
  }, [nodes, edges, snapshotOf]);

  const applySnapshot = useCallback(
    (s: Snapshot) => {
      restoring.current = true;
      setNodes(s.nodes);
      setEdges(s.edges);
    },
    [setNodes, setEdges],
  );

  const undo = useCallback(() => {
    const prev = history.current.undo(snapshotOf(live.current.nodes, live.current.edges));
    if (prev) applySnapshot(prev);
    setHistVersion((v) => v + 1);
  }, [applySnapshot, snapshotOf]);

  const redo = useCallback(() => {
    const next = history.current.redo(snapshotOf(live.current.nodes, live.current.edges));
    if (next) applySnapshot(next);
    setHistVersion((v) => v + 1);
  }, [applySnapshot, snapshotOf]);

  // Ctrl/Cmd+Z / Shift+Z / Y — skipped entirely while a text field has focus, so undo inside a
  // param input stays the browser's own text undo, never a canvas mutation under the cursor.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) {
        return;
      }
      if (!(e.ctrlKey || e.metaKey)) return;
      const k = e.key.toLowerCase();
      if (k === "z") {
        e.preventDefault();
        if (e.shiftKey) redo();
        else undo();
      } else if (k === "y") {
        e.preventDefault();
        redo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo]);

  // "New" from the menu: a clean canvas under the prompted name. `nonce` makes a second New
  // with the same name still fire.
  useEffect(() => {
    if (!pendingNew) return;
    restoring.current = true;
    setNodes([]);
    setEdges([]);
    setViewport({ x: 0, y: 0, zoom: 1 });
    setFlowName(pendingNew.name);
    history.current.reset({ nodes: [], edges: [] });
    setSettings(DEFAULT_SETTINGS);
    setSavedKey(savedKeyOf([], [], DEFAULT_SETTINGS));
    setSaveMsg("");
    resetProjectionView();
    setHistVersion((v) => v + 1);
  }, [pendingNew, setNodes, setEdges, setViewport, resetProjectionView]);

  const onSave = useCallback(async () => {
    let name = flowName;
    if (!name) {
      name = await dialog.prompt("Flow name (letters, digits, - and _ only):");
      if (!name) return;
      setFlowName(name);
    }
    let units: string[];
    try {
      const roots = [
        ...nodes.flatMap((n) => (n.data.body.kind === "Composite" ? [n.data.body.hash] : [])),
      ];
      units = closureOf(roots, (h) => ev.get_unit(h) as WUnit).map((h) =>
        toHex(buildPreimage(ev.get_unit(h) as WUnit)),
      );
    } catch (e) {
      setSaveMsg(`not saved: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    const doc: FlowDocument = {
      nodes: nodes as unknown[],
      edges: edges as unknown[],
      viewport: getViewport(),
      units,
      server: settings,
    };
    const key = savedKeyOf(nodes, edges, settings);
    saveFlow(name, doc)
      .then((entry) => {
        setSavedKey(key);
        setSaveMsg(`saved ${new Date(entry.saved_at_epoch_seconds * 1000).toLocaleTimeString()}`);
      })
      .catch((e: unknown) => setSaveMsg(e instanceof Error ? e.message : String(e)));
  }, [flowName, nodes, edges, getViewport, ev, settings, dialog]);

  const onOpenFlow = useCallback(
    async (rec: FlowRecord) => {
      const discard = `Discard unsaved changes to "${flowName ?? "untitled"}" and open "${rec.name}"?`;
      if (dirty && !(await dialog.confirm(discard, "Discard"))) {
        return;
      }
      const ns = rec.document.nodes as PrimitiveNodeType[];
      const es = rec.document.edges as Edge[];
      // Put the flow's composites back into the workspace before anything can reference them.
      // A preimage is re-hashed on insert, so a tampered entry can only add a unit nobody
      // points at, never stand in for one.
      let openMsg = "";
      try {
        for (const hex of rec.document.units ?? []) ev.insert(fromHex(hex));
      } catch (e) {
        openMsg = `some saved units could not be restored: ${e instanceof Error ? e.message : String(e)}`;
      }
      const missing = ns.filter((n) => {
        if (n.data.body.kind !== "Composite") return false;
        try {
          ev.get_unit(n.data.body.hash);
          return false;
        } catch {
          return true;
        }
      });
      if (missing.length > 0) {
        openMsg = `${missing.length} grouped node(s) can't be resolved (saved before composites were stored) — rebuild them, then save`;
      }
      restoring.current = true;
      setNodes(ns);
      setEdges(es);
      setViewport(rec.document.viewport);
      setFlowName(rec.name);
      history.current.reset(snapshotOf(ns, es));
      const server: FlowSettings = {
        port: rec.document.server?.port ?? DEFAULT_SETTINGS.port,
        interval: rec.document.server?.interval ?? DEFAULT_SETTINGS.interval,
      };
      setSettings(server);
      setSavedKey(savedKeyOf(ns, es, server));
      // A loaded flow's node ids carry the counters they were minted with — the fresh idRef
      // must start PAST all of them, or the next drop would mint a colliding id.
      let max = -1;
      for (const n of ns) {
        for (const m of n.id.matchAll(/\d+/g)) {
          max = Math.max(max, Number(m[0]));
        }
      }
      idRef.current = max + 1;
      setSaveMsg(openMsg);
      resetProjectionView();
      setHistVersion((v) => v + 1);
      setPage("canvas");
    },
    [setNodes, setEdges, setViewport, setPage, snapshotOf, ev, resetProjectionView, dirty, flowName, dialog],
  );

  const byName = useMemo(() => {
    const m = new Map<string, WUnit>();
    for (const u of catalog) {
      const n = primitiveName(u);
      if (n) m.set(n, u);
    }
    return m;
  }, [catalog]);

  const onDragOver = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
  }, []);

  const onDrop = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      e.preventDefault();
      const name = e.dataTransfer.getData(DND_MIME);
      if (isTemplate(name)) {
        const params = templateDefaults(name);
        const iface = templateInterface(name, params);
        const node: PrimitiveNodeType = {
          id: `${name}-${idRef.current++}`,
          type: "primitive",
          position: screenToFlowPosition({ x: e.clientX, y: e.clientY }),
          data: {
            name,
            body: { kind: "Template" },
            inputs: iface.inputs,
            outputs: iface.outputs,
            params,
            variadic: false,
            minInputs: 0,
            hiddenInputs: iface.hiddenInputs,
          },
        };
        setNodes((ns) => ns.concat(node));
        return;
      }
      const u = name ? byName.get(name) : undefined;
      if (!u || !name) return;

      const position = screenToFlowPosition({ x: e.clientX, y: e.clientY });
      const inputs = u.inputs ?? [];
      const data: PrimitiveData = {
        name,
        body: { kind: "Primitive" },
        inputs,
        outputs: u.outputs ?? [],
        params: u.params ?? [],
        // The catalog's own declared arity is the floor a variadic node may shrink back to —
        // never below what core actually declared, derived here rather than hardcoded, so a
        // future catalog change to one of these leaves' default arity can't silently drift from
        // what the "−" button enforces.
        variadic: variadic.has(name),
        minInputs: inputs.length,
      };
      const node: PrimitiveNodeType = {
        id: `${name}-${idRef.current++}`,
        type: "primitive",
        position,
        data,
      };
      setNodes((ns) => ns.concat(node));
    },
    [byName, screenToFlowPosition, setNodes],
  );

  // A group of canvas nodes as core children plus the wires among them, by index.
  const groupUnits = useCallback(
    (group: PrimitiveNodeType[], es: Edge[]) => {
      const indexOf = new Map(group.map((n, i) => [n.id, i]));
      const children = group.map((n) => unitFor(n, ev, byName));
      const wedges: WEdge[] = es
        .filter((e) => indexOf.has(e.source) && indexOf.has(e.target))
        .map((e) => {
          const from = indexOf.get(e.source)!;
          const to = indexOf.get(e.target)!;
          return {
            from_child: from,
            from_port: group[from].data.outputs.findIndex((p) => p.name === e.sourceHandle),
            to_child: to,
            to_port: group[to].data.inputs.findIndex((p) => p.name === e.targetHandle),
          };
        });
      return { children, wedges };
    },
    [ev, byName],
  );

  // The server this flow declares, derived from the canvas: each connected group holding a
  // request node is one route (derive.ts). Each group is projected through core like a
  // selection being grouped, narrowed to the request input and the response/effect outputs,
  // and wrapped as an endpoint. Recomputed when the canvas's structure changes, not on moves.
  const derived = useMemo(() => {
    const { nodes: ns, edges: es } = live.current;
    const { routes, errors } = findRoutes(ns, es);
    const endpoints: EndpointDraft[] = [];
    for (const r of routes) {
      const label = `${r.method} ${r.path}`;
      try {
        const group = r.nodeIds.map((id) => ns.find((n) => n.id === id)!);
        const { children, wedges } = groupUnits(group, es);
        const spec = routeSpec(ev.propose_encapsulation(children, wedges) as WEncapSpec, group, label);
        if (typeof spec === "string") {
          errors.push(spec);
          continue;
        }
        const handler = ev.freeze_encapsulation(children, wedges, spec) as WUnit;
        const handlerHash = ev.insert(buildPreimage(handler));
        const unit = endpointUnit(handlerHash, handler.inputs ?? [], handler.outputs ?? [], r.method, r.path);
        if (typeof unit === "string") {
          errors.push(`${label}: ${unit}`);
          continue;
        }
        endpoints.push({ method: r.method, path: r.path, hash: ev.insert(buildPreimage(unit)) });
      } catch (err) {
        errors.push(`${label}: ${isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err)}`);
      }
    }
    // The server unit itself, once every route is sound and the port is a port.
    const port = Number(settings.port);
    const serverHash =
      endpoints.length > 0 && errors.length === 0 && Number.isInteger(port) && port >= 1 && port <= 65535
        ? ev.insert(buildPreimage(serverUnit(endpoints.map((e) => e.hash), port)))
        : null;
    return { endpoints, errors, serverHash };
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, groupUnits, settings.port]);

  // A wire is legal iff the upstream output type and the downstream input type are core-compatible,
  // and the input isn't already fed. The type rule is core's — `compatible` bridges to
  // `validate::compatible`, the canvas never re-decides it (I1). The single-source guard is UI
  // bookkeeping over our own edge state; core's `validate` stays the real gate at projection.
  //
  // srcPort is resolved from `sourceHandle` (P4.7: outputs are a Vec<Port>, symmetric with inputs),
  // the same way dstPort already resolves from `targetHandle` — not just the first output, since a
  // node may eventually declare more than one.
  const isValidConnection = useCallback(
    (c: Connection | Edge): boolean => {
      const source = getNode(c.source);
      const target = getNode(c.target);
      if (!source || !target) return false;
      const srcPort = source.data.outputs.find((p) => p.name === c.sourceHandle);
      const dstPort = target.data.inputs.find((p) => p.name === c.targetHandle);
      if (!srcPort || !dstPort) return false;
      const alreadyFed = getEdges().some(
        (e) => e.target === c.target && e.targetHandle === c.targetHandle,
      );
      if (alreadyFed) return false;
      return compatible(srcPort.ty, dstPort.ty);
    },
    [getNode, getEdges],
  );

  // The wire carries a type: colour it by the upstream output's hue — the same legend the ports use.
  // Resolved from `sourceHandle`, the same as isValidConnection above — not always the first output.
  const onConnect = useCallback(
    (c: Connection) => {
      const srcPort = getNode(c.source)?.data.outputs.find((p) => p.name === c.sourceHandle);
      const srcTy = srcPort?.ty ?? null;
      const edge = { ...c, style: { stroke: outputColor(srcTy), strokeWidth: 1.6 } };
      setEdges((es) => addEdge(edge, es));
    },
    [getNode, setEdges],
  );

  // ── Projection ────────────────────────────────────────────────────────────────────────────────
  // On demand, turn the wired canvas into a real core composite and show its content hash. The editor
  // only marshals STRUCTURE — children by index, edges by index, handle name → port index — then
  // hands it to core's default_spec + derive_and_freeze (behind Evaluator.project), which derive the
  // boundary, validate, and freeze. The editor re-decides none of it (one decider). Cosmetic move ⇒
  // same hash; rewire ⇒ new hash; an ambiguous or invalid graph ⇒ a surfaced error, never a guess.
  //
  // A composite's promoted inputs (`ports`) and the values currently supplied for them (`values`,
  // same length/order, `null` = unsupplied) are held alongside the frozen preimage (`pre`) so a
  // boundary field's onChange can re-eval without re-projecting. `source[i]` is which PLACED NODE
  // promoted input `i` actually came from — read directly off the composite's own boundary edges
  // (`from_child === BOUNDARY`), not guessed: core already computes this mapping and returns it,
  // the editor just wasn't reading `composite.body` before. A CLOSED composite is just the
  // zero-input case of this same shape — `ports: []`, nothing special-cased.
  //
  // `outputs`/`resultSource`/`results` are the exact dual, on the output side (P4.3, fork 2):
  // `default_spec` now proposes every free output automatically, so `results` may hold several
  // values, not one — a composite with zero free outputs is likewise just the empty case of this
  // same shape (`outputs: []`), not an error state.

  // ── Deploy / manage (holon-server) ──────────────────────────────────────────────────────
  // Everything above this point (project, group, expand, inspect) is purely local — an
  // in-browser wasm workspace. This is the first thing in the editor that talks to anything
  // outside the browser at all: holon-server, over plain HTTP, localhost-only. `deployments`
  // is the server's own live table, re-fetched after every mutation here rather than guessed
  // at locally, so this view can never drift from what the server actually has running.
  const [deployments, setDeployments] = useState<DeploymentEntry[]>([]);
  const [deployBusy, setDeployBusy] = useState(false);
  const [deployError, setDeployError] = useState("");
  // Per-slug trigger body input and last result/error — a small map rather than one shared
  // field, since more than one deployed flow can be sitting in the list at once, each with
  // its own thing to try.
  const [triggerInputs, setTriggerInputs] = useState<Record<string, string>>({});
  const [triggerResults, setTriggerResults] = useState<
    Record<string, { ok: true; result: TriggerResult } | { ok: false; message: string }>
  >({});

  const refreshDeployments = useCallback(() => {
    listDeployments()
      .then(setDeployments)
      .catch((e: unknown) => setDeployError(e instanceof ServerError ? e.message : String(e)));
  }, []);

  // Fetched at start and then every few seconds, so the header's live status (and the deployed
  // page) also follow changes made elsewhere: another tab, curl, a restart.
  useEffect(() => {
    refreshDeployments();
    const timer = setInterval(refreshDeployments, 4000);
    return () => clearInterval(timer);
  }, [refreshDeployments]);

  // Opening the Deployed page always shows the live table, not whatever the last fetch saw.
  useEffect(() => {
    if (page === "deployed") refreshDeployments();
  }, [page, refreshDeployments]);

  // ── Deploy: one action for the whole flow, named after it (DeployBar in the header) ──────
  // A flow with routes deploys as its derived server; any other flow as the whole canvas
  // projected into one unit (a clock or ingress flow). Before anything is sent, the person sees
  // exactly what it may touch (the server's consent gate checks the same list), in the dialog.
  const deployedEntry = flowName ? deployments.find((d) => d.slug === flowName) : undefined;
  const outOfDate = deployedEntry && derived.serverHash ? deployedEntry.unit !== derived.serverHash : null;

  const onDeployFlow = useCallback(async () => {
    setDeployError("");
    let name = flowName;
    if (!name) {
      name = await dialog.prompt("Name this flow first (letters, digits, - and _ only). Its deployment takes the name:");
      if (!name) return;
      setFlowName(name);
    }
    let hash: string;
    let isServer = false;
    try {
      if (derived.errors.length > 0) throw new Error(derived.errors.join("; "));
      if (derived.endpoints.length > 0) {
        if (!derived.serverHash) throw new Error(`"${settings.port}" is not a port (set it under ⚙)`);
        hash = derived.serverHash;
        isServer = true;
      } else {
        const { nodes: ns, edges: es } = live.current;
        if (ns.length === 0) throw new Error("nothing to deploy: the canvas is empty");
        const { children, wedges } = groupUnits(ns, es);
        const spec = ev.propose_encapsulation(children, wedges) as WEncapSpec;
        hash = ev.insert(buildPreimage(ev.freeze_encapsulation(children, wedges, spec) as WUnit));
      }
    } catch (err) {
      setDeployError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      return;
    }
    const caps = (ev.capabilities(hash) as WCapability[]).map(capabilityAcknowledgeName);
    let interval: number | undefined;
    if (!isServer && caps.includes("Clock")) {
      interval = Number(settings.interval);
      if (!Number.isInteger(interval) || interval < 1) {
        setDeployError("this flow reads the clock: set how often it fires under ⚙ (whole seconds, at least 1)");
        return;
      }
    }
    const existing = deployments.some((d) => d.slug === name);
    const question =
      `${existing ? "Update" : "Deploy"} "${name}"?` +
      (caps.length > 0 ? ` It may touch: ${caps.join(", ")}.` : "") +
      (existing ? " Its state and sessions are kept." : "");
    if (!(await dialog.confirm(question, existing ? "Update" : "Deploy"))) return;
    setDeployBusy(true);
    try {
      await stageClosure(ev, hash, new Set());
      await deployUnit(name, hash, caps, interval);
      refreshDeployments();
    } catch (err) {
      setDeployError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeployBusy(false);
    }
  }, [flowName, dialog, derived, settings, groupUnits, ev, deployments, refreshDeployments]);

  const onFlowAction = useCallback(
    (action: "run" | "pause" | "stop") => {
      if (!flowName) return;
      setDeployError("");
      changeDeployment(flowName, action)
        .then(() => refreshDeployments())
        .catch((e: unknown) => setDeployError(e instanceof Error ? e.message : String(e)));
    },
    [flowName, refreshDeployments],
  );

  const onDeleteDeployment = useCallback(
    async (slug: string) => {
      // Undeploying is also the one way a deployment's state is discarded (a redeploy keeps it).
      const question = `Undeploy "${slug}"? Its stored state is deleted too. This can't be undone.`;
      if (!(await dialog.confirm(question, "Undeploy"))) return;
      deleteDeployment(slug)
        .then(() => refreshDeployments())
        .catch((e: unknown) => setDeployError(e instanceof ServerError ? e.message : String(e)));
    },
    [refreshDeployments, dialog],
  );

  const onTriggerDeployment = useCallback(
    (slug: string) => {
      const body = triggerInputs[slug] ?? "";
      triggerDeployment(slug, body)
        .then((result) => setTriggerResults((r) => ({ ...r, [slug]: { ok: true, result } })))
        .catch((e: unknown) =>
          setTriggerResults((r) => ({
            ...r,
            [slug]: { ok: false, message: e instanceof ServerError ? e.message : String(e) },
          })),
        )
        // The server bumps trigger_count/error_count on every attempt, success or failure
        // alike — refreshed here unconditionally (a finally, not just the success branch), the
        // same "always resync after touching the server" shape onDeployFlow already uses for its
        // own busy flag. Without this, the count showing in the panel was real server state
        // gone stale the instant a trigger fired, not an actually-wrong count server-side.
        .finally(() => refreshDeployments());
    },
    [triggerInputs, refreshDeployments],
  );


  const onProject = useCallback(() => {
    // Hoisted out of the try block on purpose: TypeMismatch only carries an `edge` index, and
    // resolving it back to actual node ids (in the catch block below) needs this same array —
    // a `const` declared inside `try {}` isn't visible inside its own `catch {}` in JS.
    let wedges: WEdge[] = [];
    try {
      // Child index = position in `nodes` — arbitrary, because core canonicalizes child order, so a
      // cosmetic reorder cannot move the hash (I5). `id`/`position` are dropped here.
      const indexOf = new Map(nodes.map((n, i) => [n.id, i]));

      // Each node's own hash — a fresh primitive rebuild or a grouped composite reference
      // (P4.3.1), resolved identically to onGroup below via the shared unitFor. Pinned child
      // hashes (I3), index order.
      const children = nodes.map((n) => unitFor(n, ev, byName));

      // Canvas wire → core edge by index. from_port is the named output's position in the source's
      // interface, to_port the named input's position in the target's — symmetric (P4.7: outputs are
      // a Vec<Port>, no longer always port 0), the editor holds both interfaces already.
      wedges = edges.map((e) => {
        const from = indexOf.get(e.source);
        const to = indexOf.get(e.target);
        if (from === undefined || to === undefined) {
          throw new Error("a wire references an unknown node");
        }
        const fromPort = nodes[from].data.outputs.findIndex((p) => p.name === e.sourceHandle);
        if (fromPort < 0) throw new Error(`unknown source port "${String(e.sourceHandle)}"`);
        const toPort = nodes[to].data.inputs.findIndex((p) => p.name === e.targetHandle);
        if (toPort < 0) throw new Error(`unknown target port "${String(e.targetHandle)}"`);
        return { from_child: from, from_port: fromPort, to_child: to, to_port: toPort };
      });

      // Core proposes the boundary (default_spec) and — unedited — freezes it
      // (derive_and_freeze), matching project()'s combined behavior exactly for anyone who
      // never excludes anything. Split apart (not project() itself, which stays untouched
      // below for anyone who calls it directly) so excludeOutput below can re-freeze with
      // fewer exposed outputs without re-deriving the proposal from scratch (P4.3, fork 3).
      // An EncapError — cycle, type mismatch, an unresolvable child — throws the structured
      // error object itself (see WEncapError), not a generic Error, so the catch block below
      // can point at the actual node(s) at fault.
      const spec = ev.propose_encapsulation(children, wedges) as WEncapSpec;
      const composite = ev.freeze_encapsulation(children, wedges, spec) as WUnit;
      const pre = buildPreimage(composite);
      const hash = ev.insert(pre);

      // Run it through the one Evaluator, starting from each promoted input's zero value (or `null`
      // for a non-scalar port — "wire in", nothing to default to). A closed graph is `ports: []`,
      // `ev.eval(pre, [])` — exactly its old behaviour, just no longer a separate branch. `results`
      // now holds one slot per declared output (P4.3, fork 2 — default_spec proposes every free
      // output automatically); `null` in a slot ⇒ that output isn't ready (an unfilled non-scalar
      // port, a divide-by-zero, an unsupplied driver), independently of its siblings — a sink
      // graph yields the Effect it constructs (pure eval doesn't fire it).
      const ports = composite.inputs ?? [];
      // Which PLACED NODE each promoted input actually came from — read directly off the
      // composite's own boundary edges (from_child === BOUNDARY), not guessed and not
      // re-derived: derive_and_freeze already computed exactly this mapping and returned it in
      // composite.body; the editor was only reading composite.inputs/.output before and
      // discarding the rest. `nodes[to_child]` is safe because `children` (above) was built from
      // `nodes` in the same order, so the indices line up by construction.
      const boundaryEdges = composite.body.kind === "Composite" ? composite.body.edges : [];
      const source: (string | null)[] = ports.map((_, i) => {
        const edge = boundaryEdges.find((e) => e.from_child === BOUNDARY && e.from_port === i);
        return edge ? (nodes[edge.to_child]?.data.name ?? null) : null;
      });
      const values = ports.map((p) => defaultBoundaryValue(p.ty));

      // The output side (P4.3, fork 2): default_spec now proposes every free output, so
      // `composite.outputs` may hold several — symmetric with `ports`/`source` above, just
      // with the boundary edge direction reversed (to_child === BOUNDARY, the dual of
      // from_child === BOUNDARY for inputs, exactly as derive_and_freeze's own comment
      // already calls the boundary edges "the dual of how eval reads them"). Reusing
      // boundaryLabels below rather than writing a second disambiguation function — it was
      // already fully generic, never actually input-specific.
      const outputs = composite.outputs ?? [];
      const resultSource: (string | null)[] = outputs.map((_, i) => {
        const edge = boundaryEdges.find((e) => e.to_child === BOUNDARY && e.to_port === i);
        return edge ? (nodes[edge.from_child]?.data.name ?? null) : null;
      });
      const results = ev.eval(pre, values) as (WValue | null)[];
      setProjection({
        hash, pre, ports, source, values, outputs, resultSource, results, children, wedges, spec, basis: canvasKey,
      });
      setProjError("");
      setErrorNodeIds(new Set());
    } catch (err) {
      setProjection(null);
      setProjError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      // Highlighting is a nicety layered on top of the (already-set) error message — never let
      // it break error display itself if a shape ever slips past isWEncapError unexpectedly.
      try {
        const indices = isWEncapError(err) ? encapErrorNodeIndices(err, wedges) : [];
        const ids = indices.map((i) => nodes[i]?.id).filter((id): id is string => id !== undefined);
        setErrorNodeIds(new Set(ids));
      } catch {
        setErrorNodeIds(new Set());
      }
    }
  }, [nodes, edges, ev, canvasKey, byName]);

  // Group the current selection into one placed composite node (P4.3.1). React Flow's own
  // selection (shift-click / drag-box) is already live and already used for delete — this is
  // its first use for anything else. Mechanically the same derivation as onProject, just fed a
  // subset: only the selected nodes become children, only edges where BOTH endpoints are
  // selected become the composite's inner wiring — an edge with exactly one endpoint outside
  // the selection is, from the selected subgraph's own perspective, indistinguishable from an
  // ordinary free input or output, so default_spec's existing proposal derives the right
  // boundary with no new core or bridge logic. Uses the full automatic proposal directly, same
  // as onProject's own default path — no exclude-review step for grouping in this inch (fork
  // 3's own scoped-down UI stays a whole-canvas-projection feature, not extended here).
  // Deliberately does NOT include expand-to-inspect — its own separate, later inch once
  // grouping itself is proven, the same staged discipline the encapsulation work already used.
  const onGroup = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length < 2) return; // mirrors the button's own disabled guard, defensively
    const selectedIds = new Set(selected.map((n) => n.id));
    const indexOf = new Map(selected.map((n, i) => [n.id, i]));

    let wedges: WEdge[] = [];
    try {
      const children = selected.map((n) => unitFor(n, ev, byName));

      const innerEdges = edges.filter((e) => selectedIds.has(e.source) && selectedIds.has(e.target));
      wedges = innerEdges.map((e) => {
        const from = indexOf.get(e.source)!;
        const to = indexOf.get(e.target)!;
        const fromPort = selected[from].data.outputs.findIndex((p) => p.name === e.sourceHandle);
        const toPort = selected[to].data.inputs.findIndex((p) => p.name === e.targetHandle);
        return { from_child: from, from_port: fromPort, to_child: to, to_port: toPort };
      });

      const composite = ev.project(children, wedges) as WUnit;
      const pre = buildPreimage(composite);
      const hash = ev.insert(pre);

      const boundaryEdges = composite.body.kind === "Composite" ? composite.body.edges : [];
      const rawInputs = composite.inputs ?? [];
      const rawOutputs = composite.outputs ?? [];

      // Which (selected-node, port) each promoted input / exposed output came from — read
      // directly off the boundary edges core already computed, not guessed. Purely
      // index-based (from_port/to_port against BOUNDARY), so this works on the RAW,
      // possibly-colliding ports straight from ev.project — naming below doesn't change it,
      // it only feeds it.
      const promotedFrom = rawInputs.map((_, i) => {
        const be = boundaryEdges.find((e) => e.from_child === BOUNDARY && e.from_port === i);
        return be ? { child: be.to_child, port: be.to_port } : null;
      });
      const exposedFrom = rawOutputs.map((_, i) => {
        const be = boundaryEdges.find((e) => e.to_child === BOUNDARY && e.to_port === i);
        return be ? { child: be.from_child, port: be.from_port } : null;
      });

      // Name the new node's ports by provenance, reusing boundaryLabels' own algorithm rather
      // than a second, plainer disambiguation — "add · b" instead of a bare "b_2". These
      // labels become the ports' REAL name (this node's actual handle ids), not just a
      // display string layered on top: composite.inputs/outputs are renamed here, after
      // ev.project already returned and the hash is already computed, so this can never
      // affect the composite's identity — the same "renaming is purely local view-state"
      // property the plain-suffix version already had. Falls back to occurrence-counting only
      // when provenance itself doesn't separate two ports — two unconfigured instances of the
      // literally same primitive, genuinely identical — exactly boundaryLabels' own last
      // resort for the promoted-input side.
      const inputSource = promotedFrom.map((x) => (x ? selected[x.child]?.data.name ?? null : null));
      const outputSource = exposedFrom.map((x) => (x ? selected[x.child]?.data.name ?? null : null));
      const inputLabels = boundaryLabels(rawInputs, inputSource);
      const outputLabels = boundaryLabels(rawOutputs, outputSource);
      const inputs = rawInputs.map((p, i) => ({ ...p, name: inputLabels[i] }));
      const outputs = rawOutputs.map((p, i) => ({ ...p, name: outputLabels[i] }));

      const groupId = `composite-${idRef.current++}-${hash.slice(0, 8)}`;
      const centroid = {
        x: selected.reduce((s, n) => s + n.position.x, 0) / selected.length,
        y: selected.reduce((s, n) => s + n.position.y, 0) / selected.length,
      };
      const groupNode: PrimitiveNodeType = {
        id: groupId,
        type: "primitive",
        position: centroid,
        data: {
          name: hash.slice(0, 8),
          body: { kind: "Composite", hash },
          inputs,
          outputs,
          params: [],
          variadic: false,
          minInputs: 0,
        },
      };

      // Rewire every edge that crossed the selection boundary to the new node's corresponding
      // handle; drop every edge now fully internal to the frozen composite (it's baked into
      // the hash, no longer a canvas wire); leave every edge untouched by the selection exactly
      // as it was.
      const nextEdges: Edge[] = edges.flatMap((e): Edge[] => {
        const sIn = selectedIds.has(e.source);
        const tIn = selectedIds.has(e.target);
        if (sIn && tIn) return [];
        if (sIn) {
          const ci = indexOf.get(e.source)!;
          const cp = selected[ci].data.outputs.findIndex((p) => p.name === e.sourceHandle);
          const oi = exposedFrom.findIndex((x) => x?.child === ci && x?.port === cp);
          if (oi < 0) return []; // shouldn't happen against an honest proposal; drop defensively
          return [{ ...e, source: groupId, sourceHandle: outputs[oi].name }];
        }
        if (tIn) {
          const ci = indexOf.get(e.target)!;
          const cp = selected[ci].data.inputs.findIndex((p) => p.name === e.targetHandle);
          const ii = promotedFrom.findIndex((x) => x?.child === ci && x?.port === cp);
          if (ii < 0) return [];
          return [{ ...e, target: groupId, targetHandle: inputs[ii].name }];
        }
        return [e];
      });

      setNodes((ns) => [...ns.filter((n) => !selectedIds.has(n.id)), groupNode]);
      setEdges(nextEdges);
      setProjError("");
      setErrorNodeIds(new Set());
    } catch (err) {
      setProjError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      try {
        const indices = isWEncapError(err) ? encapErrorNodeIndices(err, wedges) : [];
        const ids = indices.map((i) => selected[i]?.id).filter((id): id is string => id !== undefined);
        setErrorNodeIds(new Set(ids));
      } catch {
        setErrorNodeIds(new Set());
      }
    }
  }, [nodes, edges, ev, setNodes, setEdges, byName]);

  // Expand a placed composite node back into its children (P4.3.1's last piece — the dual of
  // onGroup). One level only, deliberately: a nested composite child is revealed still
  // collapsed, as its own composite node, not recursively unpacked — the same restraint
  // grouping already has (it only ever acts on the current selection, never implicitly
  // nesting). Deep, read-only, annotated descent is P4.4.1's own, separate job.
  const onExpand = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length !== 1) return; // mirrors the button's own disabled guard, defensively
    const target = selected[0];
    if (target.data.body.kind !== "Composite") return;
    const hash = target.data.body.hash;

    try {
      const wunit = ev.get_unit(hash) as WUnit;
      // get_unit reads back exactly what derive_and_freeze produced at grouping time, which is
      // always a Composite — but WBody is a discriminated union either way, so this check is
      // required to narrow it, not just a defensive nicety.
      if (wunit.body.kind !== "Composite") throw new Error("expand: not a composite");
      const childHashes = wunit.body.children;
      const innerEdges = wunit.body.edges;

      // Reconstruct each child fresh — a primitive regains full editability (checked against
      // the same variadicPrimitives() set onDrop uses, so an expanded node is indistinguishable
      // from one just dropped from the palette), a nested composite stays a placed reference
      // (P4.3.1's own one-level rule above). No renaming/dedup needed here the way grouping
      // needed boundaryLabels: expanding SPLITS one already-frozen interface whose names were
      // already unique when it was built, it doesn't COMBINE several into one new one.
      const spacing = 220;
      const startX = target.position.x - ((childHashes.length - 1) * spacing) / 2;
      const newNodes: PrimitiveNodeType[] = childHashes.map((h, i) => {
        const cu = ev.get_unit(h) as WUnit;
        const position = { x: startX + i * spacing, y: target.position.y };
        const data: PrimitiveData =
          cu.body.kind === "Primitive"
            ? {
                name: cu.body.name,
                body: { kind: "Primitive" },
                inputs: cu.inputs ?? [],
                outputs: cu.outputs ?? [],
                params: cu.params ?? [],
                variadic: variadic.has(cu.body.name),
                // The catalog's own declared arity, not this (possibly grown-then-frozen)
                // child's current one — same floor onDrop's own placement already enforces.
                // Falls back to the child's own arity only if the catalog lookup somehow
                // misses, rather than crashing.
                minInputs: (byName.get(cu.body.name)?.inputs ?? cu.inputs ?? []).length,
              }
            : {
                name: h.slice(0, 8),
                body: { kind: "Composite", hash: h },
                inputs: cu.inputs ?? [],
                outputs: cu.outputs ?? [],
                params: [],
                variadic: false,
                minInputs: 0,
              };
        const id =
          cu.body.kind === "Primitive"
            ? `${cu.body.name}-${idRef.current++}`
            : `composite-${idRef.current++}-${h.slice(0, 8)}`;
        return { id, type: "primitive", position, data };
      });

      // Canvas edges that pointed at the composite's own ports get re-pointed at the right new
      // child's handle instead — resolved the same "read what core already computed" way
      // onGroup's own rewiring works, just walked in the opposite direction: find which
      // boundary edge core built for this port, follow it to the (child, port) it actually
      // came from.
      const rewired: Edge[] = edges.flatMap((e): Edge[] => {
        if (e.target === target.id) {
          const inIdx = target.data.inputs.findIndex((p) => p.name === e.targetHandle);
          const be = innerEdges.find((x) => x.from_child === BOUNDARY && x.from_port === inIdx);
          if (!be) return []; // shouldn't happen against an honest unit; drop defensively
          const child = newNodes[be.to_child];
          const handle = child.data.inputs[be.to_port]?.name;
          if (!handle) return [];
          return [{ ...e, target: child.id, targetHandle: handle }];
        }
        if (e.source === target.id) {
          const outIdx = target.data.outputs.findIndex((p) => p.name === e.sourceHandle);
          const be = innerEdges.find((x) => x.to_child === BOUNDARY && x.to_port === outIdx);
          if (!be) return [];
          const child = newNodes[be.from_child];
          const handle = child.data.outputs[be.from_port]?.name;
          if (!handle) return [];
          return [{ ...e, source: child.id, sourceHandle: handle }];
        }
        return [e];
      });

      // The composite's own inner wiring (both endpoints are children, not BOUNDARY) becomes
      // real canvas edges between the new nodes — through addEdge, same as onConnect, so these
      // get real ids and the same type-colored styling every other wire on the canvas has,
      // not a bare, unstyled object.
      const withInner = innerEdges
        .filter((e) => e.from_child !== BOUNDARY && e.to_child !== BOUNDARY)
        .reduce((acc: Edge[], e) => {
          const srcNode = newNodes[e.from_child];
          const tgtNode = newNodes[e.to_child];
          const srcPort = srcNode.data.outputs[e.from_port];
          const tgtHandle = tgtNode.data.inputs[e.to_port]?.name;
          if (!srcPort || !tgtHandle) return acc;
          const conn = {
            source: srcNode.id,
            sourceHandle: srcPort.name,
            target: tgtNode.id,
            targetHandle: tgtHandle,
          };
          return addEdge({ ...conn, style: { stroke: outputColor(srcPort.ty), strokeWidth: 1.6 } }, acc);
        }, rewired);

      setNodes((ns) => [...ns.filter((n) => n.id !== target.id), ...newNodes]);
      setEdges(withInner);
      setProjError("");
      setErrorNodeIds(new Set());
    } catch (err) {
      // get_unit only ever fails as a plain string (no structured error family a "not found"
      // belongs to), and every hash reached here was already resolved when the composite was
      // frozen — so there's no clean "which node is at fault" the way an EncapError has;
      // nothing to highlight, just the message.
      setProjError(err instanceof Error ? err.message : String(err));
    }
  }, [nodes, edges, ev, byName, setNodes, setEdges]);

  // Open the inspect panel for the single selected node (P4.4.1). Unlike onExpand, this never
  // touches nodes/edges — it only resolves the selected node's own hash (via unitFor, the same
  // resolution onProject/onGroup already use — a fresh primitive gets inserted if it hasn't
  // been yet, an already-frozen composite is just read off its own body.hash) and hands that
  // hash to the panel, which does its own read-only descent from there.
  const onInspect = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length !== 1) return; // mirrors the button's own disabled guard, defensively
    setInspecting(unitFor(selected[0], ev, byName));
  }, [nodes, ev, byName]);

  // A boundary field changed: update its slot and re-eval live. Safe to be reactive (unlike
  // projecting itself) because a boundary value is never part of the composite's hash — it's a
  // pure eval-time argument, not identity — so there is no "cosmetic move" noise to avoid here.
  const setBoundaryValue = useCallback(
    (index: number, value: WValue | null) => {
      setProjection((p) => {
        if (!p) return p;
        const values = p.values.map((v, i) => (i === index ? value : v));
        return { ...p, values, results: ev.eval(p.pre, values) as (WValue | null)[] };
      });
    },
    [ev],
  );

  // Exclude one currently-exposed output and re-freeze (P4.3, fork 3). Unlike setBoundaryValue
  // above, this is NOT a lightweight re-eval: excluding an output is a real authoring choice
  // that changes what the composite's interface declares, so it changes the hash too — a
  // genuine re-freeze through derive_and_freeze, not a display filter over the old one.
  // `outputs`/`resultSource` line up 1:1 with `spec.expose` by construction (derive_and_freeze
  // builds them from it in order), so filtering all three by the same index keeps them in
  // sync without re-deriving provenance from composite.body again. No "re-include": the
  // original proposal is recovered by pressing "project" again, already a single click.
  const excludeOutput = useCallback(
    (index: number) => {
      setProjection((p) => {
        if (!p) return p;
        try {
          const expose = p.spec.expose.filter((_, i) => i !== index);
          const spec: WEncapSpec = { promote: p.spec.promote, expose };
          const composite = ev.freeze_encapsulation(p.children, p.wedges, spec) as WUnit;
          const pre = buildPreimage(composite);
          const hash = ev.insert(pre);
          const outputs = p.outputs.filter((_, i) => i !== index);
          const resultSource = p.resultSource.filter((_, i) => i !== index);
          const results = ev.eval(pre, p.values) as (WValue | null)[];
          return { ...p, hash, pre, outputs, resultSource, results, spec };
        } catch {
          // Dropping one entry from an already-valid spec can't create a new validation
          // failure — nothing here should throw. If it somehow does, leave the projection
          // exactly as it was rather than silently corrupt it.
          return p;
        }
      });
    },
    [ev],
  );

  // Purely a rendering decoration for React Flow — never touches the underlying `nodes` state
  // that onProject/onDrop read and write, so flagging a node as errored has zero effect on
  // hashing or identity (the same "cosmetic vs. structural" line I5 already draws elsewhere).
  const decoratedNodes = useMemo(
    () =>
      errorNodeIds.size === 0
        ? nodes
        : nodes.map((n) =>
            errorNodeIds.has(n.id) ? { ...n, data: { ...n.data, error: true } } : n,
          ),
    [nodes, errorNodeIds],
  );

  // How many placed nodes are currently selected — React Flow's own selection state
  // (shift-click / drag-box), already live for delete, now also gating the "group" button
  // (P4.3.1). Cheap enough over a canvas-sized node list not to need memoizing.
  const selectedCount = nodes.filter((n) => n.selected).length;
  // "Expand" is the dual of "group": only meaningful for exactly one selected composite node —
  // mirrors group's own selectedCount<2 guard rather than a new per-node interaction
  // PrimitiveNode.tsx doesn't otherwise have for whole-node operations.
  const canExpand = selectedCount === 1 && nodes.some((n) => n.selected && n.data.body.kind === "Composite");

  return (
    <div className="hl-canvas" style={{ position: "relative" }} onDrop={onDrop} onDragOver={onDragOver}>
      <ReactFlow<PrimitiveNodeType>
        nodes={decoratedNodes}
        onNodesChange={onNodesChange}
        edges={edges}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        isValidConnection={isValidConnection}
        nodeTypes={nodeTypes}
        connectionLineStyle={{ stroke: "#6fd3e0", strokeWidth: 1.6 }}
        fitView={false}
        minZoom={0.4}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        // Select a node or wire, then Backspace or Delete removes it — React Flow's own built-in
        // mechanism (deleting a node also cleans up any wires attached to it; nothing hand-rolled
        // here). The default is Backspace only; Delete is added because it's the key most people
        // reach for first, especially on Windows. React Flow itself skips this handling while a
        // text input/select inside a node has focus, so editing a const's value or type is safe.
        deleteKeyCode={["Backspace", "Delete"]}
      >
        <Background variant={BackgroundVariant.Lines} gap={26} color="#1b2a45" />
        <Controls showInteractive={false} />
        <Panel position="top-left">
          {/* histVersion is what makes the undo/redo disabled state live — history itself is a
              ref (pushing must not re-render the canvas mid-drag), so the buttons re-render off
              this counter instead. */}
          <div data-hist={histVersion} style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <span style={{ color: "#9fb4d4", fontSize: 12, letterSpacing: "0.04em" }}>
              {flowName ?? "untitled"}
              {dirty && <span title="unsaved changes"> •</span>}
            </span>
            <button
              onClick={onSave}
              style={{
                font: "inherit",
                fontSize: 11,
                padding: "3px 10px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: "pointer",
              }}
            >
              save
            </button>
            <button
              onClick={undo}
              disabled={history.current.past.length === 0}
              title="undo (Ctrl+Z)"
              style={{
                font: "inherit",
                fontSize: 11,
                padding: "3px 8px",
                color: "#9fb4d4",
                background: "#0e1a2f",
                border: "1px solid #1b2a45",
                borderRadius: 3,
                cursor: history.current.past.length === 0 ? "default" : "pointer",
                opacity: history.current.past.length === 0 ? 0.4 : 1,
              }}
            >
              ⟲
            </button>
            <button
              onClick={redo}
              disabled={history.current.future.length === 0}
              title="redo (Ctrl+Y / Ctrl+Shift+Z)"
              style={{
                font: "inherit",
                fontSize: 11,
                padding: "3px 8px",
                color: "#9fb4d4",
                background: "#0e1a2f",
                border: "1px solid #1b2a45",
                borderRadius: 3,
                cursor: history.current.future.length === 0 ? "default" : "pointer",
                opacity: history.current.future.length === 0 ? 0.4 : 1,
              }}
            >
              ⟳
            </button>
            <DeployBar
              entry={deployedEntry}
              outOfDate={outOfDate}
              busy={deployBusy}
              settings={settings}
              setSettings={setSettings}
              onDeploy={onDeployFlow}
              onAction={onFlowAction}
            />
            {saveMsg && <span style={{ color: "#9ad29f", fontSize: 11 }}>{saveMsg}</span>}
            {deployError && page === "canvas" && (
              <span style={{ color: "#e0916f", fontSize: 11, maxWidth: 420 }}>{deployError}</span>
            )}
          </div>
        </Panel>
        <Panel position="top-right">
          <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-end" }}>
            <button
              onClick={onProject}
              disabled={nodes.length === 0}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: nodes.length === 0 ? "default" : "pointer",
                opacity: nodes.length === 0 ? 0.4 : 1,
              }}
            >
              project
            </button>
            <button
              onClick={onGroup}
              disabled={selectedCount < 2}
              title={selectedCount < 2 ? "select 2+ nodes to group" : `group ${selectedCount} nodes`}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: selectedCount < 2 ? "default" : "pointer",
                opacity: selectedCount < 2 ? 0.4 : 1,
              }}
            >
              group
            </button>
            <button
              onClick={onExpand}
              disabled={!canExpand}
              title={canExpand ? "expand this composite" : "select one composite node to expand"}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: canExpand ? "pointer" : "default",
                opacity: canExpand ? 1 : 0.4,
              }}
            >
              expand
            </button>
            <button
              onClick={onInspect}
              disabled={selectedCount !== 1}
              title={selectedCount === 1 ? "inspect this node" : "select one node to inspect"}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: selectedCount === 1 ? "pointer" : "default",
                opacity: selectedCount === 1 ? 1 : 0.4,
              }}
            >
              inspect
            </button>
            {projection && (
              <div style={{ fontSize: 12, color: "#9fb4d4", textAlign: "right", lineHeight: 1.4 }}>
                <div style={{ color: "#6fd3e0" }} title={projection.hash}>
                  {projection.hash.slice(0, 12)}…
                </div>
                {projection.ports.length > 0 && (
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      gap: 4,
                      margin: "6px 0",
                      alignItems: "flex-end",
                    }}
                  >
                    {boundaryLabels(projection.ports, projection.source).map((label, i) => (
                      <BoundaryField
                        key={i}
                        port={projection.ports[i]}
                        label={label}
                        value={projection.values[i]}
                        onChange={(v) => setBoundaryValue(i, v)}
                      />
                    ))}
                  </div>
                )}
                {projection.results.length > 0 && (
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      gap: 2,
                      alignItems: "flex-end",
                      color: "#9ad29f",
                    }}
                  >
                    {boundaryLabels(projection.outputs, projection.resultSource).map((label, i) => (
                      <div key={i} style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <span>
                          {label} ={" "}
                          {projection.results[i] ? formatValue(projection.results[i]!) : "not ready"}
                        </span>
                        <button
                          className="nodrag"
                          onClick={() => excludeOutput(i)}
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
            {projError && (
              <div style={{ fontSize: 12, color: "#e0916f", maxWidth: 240, textAlign: "right" }}>
                {projError}
              </div>
            )}
          </div>
        </Panel>
        <Panel position="bottom-left" style={{ marginLeft: 56 }}>
          <ServerPanel
            key={flowName ?? ""}
            ev={ev}
            endpoints={derived.endpoints}
            errors={derived.errors}
            liveSlug={deployedEntry && deployedEntry.port !== null ? (flowName ?? "") : ""}
          />
        </Panel>
        {inspecting && (
          <Panel position="bottom-right">
            <InspectPanel hash={inspecting} ev={ev} onClose={() => setInspecting(null)} />
          </Panel>
        )}
      </ReactFlow>
      {nodes.length === 0 && page === "canvas" && (
        <div className="hl-canvas__empty">drag a primitive here to place it</div>
      )}
      {/* The Flows / Deployed pages are overlays ON TOP of the always-mounted canvas — React
          Flow's own state (nodes mid-edit, viewport) lives in it and must survive navigation,
          so the canvas is covered, never unmounted. The deployments list used to live in a
          bottom-left panel on the canvas itself; it moved here wholesale — one home, not two. */}
      {page !== "canvas" && (
        <div
          style={{
            position: "absolute",
            inset: 0,
            zIndex: 10,
            background: "#0b1526",
            overflow: "auto",
            padding: 24,
          }}
        >
          {page === "flows" && <FlowsPage onOpen={onOpenFlow} />}
          {page === "deployed" && (
            <div style={{ maxWidth: 760 }}>
              <div style={{ color: "#cfe3ff", fontSize: 14, letterSpacing: "0.04em", marginBottom: 4 }}>
                deployed
              </div>
              <div style={{ color: "#9fb4d4", fontSize: 11, marginBottom: 12 }}>
                {deployments.length} deployment{deployments.length === 1 ? "" : "s"} ·{" "}
                {deployments.reduce((a, d) => a + d.trigger_count, 0)} fires ·{" "}
                {deployments.reduce((a, d) => a + d.error_count, 0)} errors
              </div>
              {deployError && (
                <div style={{ color: "#e0916f", fontSize: 12, marginBottom: 8 }}>{deployError}</div>
              )}
              {deployments.length === 0 && (
                <div style={{ color: "#9fb4d4", fontSize: 12 }}>
                  nothing deployed — project a flow on the canvas, then deploy it
                </div>
              )}
              {deployments.map((entry) => (
                <DeploymentCard
                  key={entry.slug}
                  entry={entry}
                  onAction={(action, from) =>
                    changeDeployment(entry.slug, action, from).then(() => refreshDeployments())
                  }
                  triggerInput={triggerInputs[entry.slug] ?? ""}
                  onTriggerInputChange={(v) =>
                    setTriggerInputs((t) => ({ ...t, [entry.slug]: v }))
                  }
                  onTrigger={() => onTriggerDeployment(entry.slug)}
                  onDelete={() => onDeleteDeployment(entry.slug)}
                  result={triggerResults[entry.slug]}
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Editor() {
  const dialog = useDialog();
  const [catalog, setCatalog] = useState<WUnit[] | null>(null);
  const [variadic, setVariadic] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string>("");
  const [page, setPage] = useState<AppPage>("canvas");
  // "New" carries the prompted name down to the canvas; `nonce` makes a second New with the
  // same name still take effect (the object identity changes even when the name doesn't).
  const [pendingNew, setPendingNew] = useState<{ name: string; nonce: number } | null>(null);
  // Whether the canvas has unsaved changes, reported up by Canvas so New can ask first. A ref:
  // only read when New is clicked, so it never needs to re-render the header.
  const canvasDirty = useRef(false);
  const onDirtyChange = useCallback((d: boolean) => {
    canvasDirty.current = d;
  }, []);

  useEffect(() => {
    (async () => {
      await ensureReady();
      setCatalog(primitiveCatalog());
      setVariadic(variadicPrimitives());
    })().catch((e: unknown) => setError(String(e)));
  }, []);

  const onNew = async () => {
    if (canvasDirty.current && !(await dialog.confirm("Discard unsaved changes and start a new flow?", "Discard"))) {
      return;
    }
    const name = await dialog.prompt("Flow name (letters, digits, - and _ only):");
    if (!name) return;
    setPendingNew((p) => ({ name, nonce: (p?.nonce ?? 0) + 1 }));
    setPage("canvas");
  };

  const menuBtn = (active: boolean, enabled = true): CSSProperties => ({
    font: "inherit",
    fontSize: 12,
    letterSpacing: "0.04em",
    padding: "3px 10px",
    color: active ? "#0e1a2f" : "#9fb4d4",
    background: active ? "#6fd3e0" : "transparent",
    border: "1px solid #1b2a45",
    borderRadius: 3,
    cursor: enabled ? "pointer" : "default",
    opacity: enabled ? 1 : 0.4,
  });

  return (
    <div className="hl-app">
      <header className="hl-header">
        <span className="hl-header__brand">holon</span>
        <span className="hl-header__sep">·</span>
        <span className="hl-header__mode">authoring</span>
        <nav style={{ display: "flex", gap: 6, marginLeft: 18 }}>
          <button style={menuBtn(false)} onClick={onNew}>
            new
          </button>
          <button style={menuBtn(page === "flows")} onClick={() => setPage(page === "flows" ? "canvas" : "flows")}>
            flows
          </button>
          <button
            style={menuBtn(page === "deployed")}
            onClick={() => setPage(page === "deployed" ? "canvas" : "deployed")}
          >
            deployed
          </button>
          <button style={menuBtn(false, false)} disabled title="deferred — the flow marketplace lands later">
            marketplace
          </button>
        </nav>
        <span className="hl-header__count">
          {catalog ? `${catalog.length} genesis atoms` : "loading core…"}
        </span>
      </header>

      {error ? (
        <pre className="hl-error">{error}</pre>
      ) : (
        <div className="hl-body">
          {catalog && <Palette catalog={catalog} templates={TEMPLATES} />}
          <ReactFlowProvider>
            {catalog && (
              <Canvas
                catalog={catalog}
                variadic={variadic}
                page={page}
                setPage={setPage}
                pendingNew={pendingNew}
                onDirtyChange={onDirtyChange}
              />
            )}
          </ReactFlowProvider>
        </div>
      )}
    </div>
  );
}

export default function App() {
  return (
    <DialogProvider>
      <Editor />
    </DialogProvider>
  );
}