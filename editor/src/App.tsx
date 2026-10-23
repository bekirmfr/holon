// The authoring canvas: a palette (derived from core's catalog) on the left, a blueprint canvas on
// the right. Drag a primitive across to place a typed node. Nothing here re-asserts a primitive's
// shape; every placed node reads its ports from the catalog interface (I1). The canvas's own
// operations live in canvasOps.ts, its panels in CanvasPanels.tsx, the pages over it in Pages.tsx,
// and its undo history and deployment table in their hooks.

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
  primitiveDocs,
  variadicPrimitives,
  compatible,
  buildPreimage,
  Evaluator,
  type WUnit,
  type WValue,
  type WEncapSpec,
  manifestItems,
  manifestTrigger,
} from "./holon";
import {
  deployUnit,
  previewDeploy,
  changeDeployment,
  getDeployedDocument,
  getFlow,
  saveFlow,
  listFlows,
  type FlowDocument,
  type FlowRecord,
} from "./server";
import { outputColor } from "./theme";
import { Palette, DND_MIME, FLOW_PREFIX } from "./Palette";
import { PrimitiveNode, type PrimitiveNodeType } from "./PrimitiveNode";
import { InspectPanel } from "./InspectPanel";
import { documentKey, structuralKey } from "./flowDoc";
import { documentUnits, nameOf, rememberName, restoreDocument } from "./docUnits";
import { DialogProvider, useDialog } from "./Dialog";
import { ServerPanel } from "./ServerPanel";
import { DeployBar, DEFAULT_SETTINGS, type FlowSettings } from "./DeployBar";
import { TEMPLATES } from "./templates";
import { isWEncapError, describeItem } from "./diagnostics";
import { primitiveName, stageClosure, unitFor } from "./canvasUnits";
import {
  deriveServer,
  excludeProjectedOutput,
  flowAsNode,
  freezeInner,
  innerCanvas,
  nodeFromPalette,
  replaceComposite,
  flowUnitHash,
  expandComposite,
  groupOf,
  groupSelection,
  projectCanvas,
  traceOfUnit,
  type Projection,
} from "./canvasOps";
import { FlowTabs, FlowToolbar, ProjectionPanel } from "./CanvasPanels";
import { DeployedPage, FlowsPage, type AppPage } from "./Pages";
import { newHistory, snapshotOf, useCanvasHistory } from "./useCanvasHistory";
import { useDeployments } from "./useDeployments";
import { SAMPLES, buildSample, type Sample } from "./samples";
import { blankMeta, blankTab, metaSetter, nextAfterClose, tabKind, type FlowMeta, type Mode, type TabState } from "./tabs";
import { NodeActions, NodeDocs, nodeDescriber } from "./NodeDocs";
import { AccessButton } from "./AccessButton";
import { useViewMode, ViewModePanes } from "./useViewMode";
import { CompositeOverlay, type FrozenOk, type OverlayFrame, type OverlayOps, type Renames } from "./CompositeOverlay";
import type { Node as FlowNode } from "@xyflow/react";
import { innerSteps } from "./replay";

// What a save records, for unsaved-change tracking: the canvas plus the server draft.
const savedKeyOf = (ns: PrimitiveNodeType[], es: Edge[], settings: FlowSettings) =>
  documentKey(ns, es) + JSON.stringify(settings);

// A new, empty flow's tab: nothing to save until something is placed.
const freshMeta = (name: string | null): FlowMeta => ({ ...blankMeta(name), savedKey: savedKeyOf([], [], DEFAULT_SETTINGS) });
const freshTab = (name: string | null): TabState => ({ ...blankTab(name), meta: freshMeta(name) });

const nodeTypes = { primitive: PrimitiveNode } as unknown as NodeTypes;

function Canvas({
  catalog,
  variadic,
  page,
  setPage,
  pendingNew,
  docs,
}: {
  catalog: WUnit[];
  variadic: Set<string>;
  page: AppPage;
  setPage: (p: AppPage) => void;
  pendingNew: { name: string; nonce: number } | null;
  docs: Map<string, string>;
}) {
  const dialog = useDialog();
  const [nodes, setNodes, onNodesChange] = useNodesState<PrimitiveNodeType>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { screenToFlowPosition, getNode, getEdges, getViewport, setViewport, fitView } =
    useReactFlow<PrimitiveNodeType>();
  const idRef = useRef(0);
  // The one in-browser workspace every placed composite lives in (see Projection below).
  // Declared first because saving and opening a flow read and fill it too.
  const ev = useMemo(() => new Evaluator(), []);
  // Projection state, declared here (not with the Projection section below) because New and
  // Open reset it. See that section for what each field means.
  // Projection state, declared here because New and Open reset it (see canvasOps.ts).
  const [projection, setProjection] = useState<Projection | null>(null);
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

  // The last deploy or deployment error, shown in the toolbar and on the deployed page.
  const [deployError, setDeployError] = useState("");

  // New and Open start from a clean slate: no projection, error highlight or inspect panel
  // carried over from the previous flow.
  const resetProjectionView = useCallback(() => {
    setProjection(null);
    setProjError("");
    setErrorNodeIds(new Set());
    setInspecting(null);
    setDeployError("");
  }, []);

  // ── the current flow (P4.8) ─────────────────────────────────────────────────────────────
  // Everything the active tab holds about its flow besides the canvas (tabs.ts's FlowMeta), as
  // one state: its name, whether it's saved, its deploy settings (⚙), what it was last saved as
  // (unsaved changes: the canvas differs from that), and view mode's deployment, version, run,
  // step and node. Switching tabs captures and restores it whole, so a field added to FlowMeta
  // needs nothing here. A saved flow is a named workspace document — nodes/edges/viewport
  // exactly as left, stored opaquely by the server (server.ts's FlowDocument), apart from the
  // content-addressed unit the same canvas projects to: presentation stays out of identity.
  const [flow, setFlow] = useState<FlowMeta>(() => freshMeta(null));
  const { name: flowName, settings, savedKey, saveMsg, viewing } = flow;
  const setFlowName = useMemo(() => metaSetter(setFlow, "name"), []);
  const setSavedOnServer = useMemo(() => metaSetter(setFlow, "saved"), []);
  const setSettings = useMemo(() => metaSetter(setFlow, "settings"), []);
  const setSavedKey = useMemo(() => metaSetter(setFlow, "savedKey"), []);
  const setSaveMsg = useMemo(() => metaSetter(setFlow, "saveMsg"), []);
  const setViewing = useMemo(() => metaSetter(setFlow, "viewing"), []);
  const setViewUnit = useMemo(() => metaSetter(setFlow, "viewUnit"), []);
  const setViewTrace = useMemo(() => metaSetter(setFlow, "viewTrace"), []);
  const setSelectedRun = useMemo(() => metaSetter(setFlow, "selectedRun"), []);
  const setStep = useMemo(() => metaSetter(setFlow, "step"), []);
  const setInspectNode = useMemo(() => metaSetter(setFlow, "inspectNode"), []);
  const beforeView = useRef<TabState["beforeView"]>(null);
  // The deployed document shown in view mode: what edit mode starts from when there is no saved
  // flow of that name.
  const viewDoc = useRef<FlowDocument | null>(null);
  const { history, restoring, histVersion, bump, undo, redo } = useCanvasHistory(
    nodes,
    edges,
    setNodes,
    setEdges,
    viewing !== null,
  );
  // The canvas as of this render, for callbacks that read it without depending on it.
  const live = useRef({ nodes, edges });
  live.current = { nodes, edges };
  const dirty = useMemo(
    // While viewing, what counts is whether the flow set aside had unsaved changes.
    () => (viewing ? (beforeView.current?.dirty ?? false) : savedKeyOf(nodes, edges, settings) !== savedKey),
    [nodes, edges, settings, savedKey, viewing],
  );

  // ── Tabs: one per open flow (tabs.ts) ─────────────────────────────────────────────────────
  // The canvas shows the active tab; the others are kept whole in `stash` and put back when
  // picked. A flow is open in one tab at most: opening it again goes to its tab.
  const [tabIds, setTabIds] = useState<number[]>([0]);
  const [activeId, setActiveId] = useState(0);
  const stash = useRef(new Map<number, TabState>());
  const nextTabId = useRef(1);

  const captureTab = useCallback(
    (): TabState => ({
      meta: flow,
      nodes: live.current.nodes,
      edges: live.current.edges,
      viewport: getViewport(),
      history: history.current,
      idCounter: idRef.current,
      beforeView: beforeView.current,
      viewDoc: viewDoc.current,
      dirty,
    }),
    [flow, getViewport, history, dirty],
  );

  const applyTab = useCallback(
    (t: TabState) => {
      restoring.current = true;
      setNodes(t.nodes);
      setEdges(t.edges);
      setViewport(t.viewport);
      history.current = t.history;
      idRef.current = t.idCounter;
      beforeView.current = t.beforeView;
      viewDoc.current = t.viewDoc;
      setFlow(t.meta);
      resetProjectionView();
      bump();
    },
    [restoring, history, setNodes, setEdges, setViewport, resetProjectionView, bump],
  );

  /** Show `t` in tab `id`, or in a new tab (null) unless the active one is an empty, untouched,
   *  unnamed draft, which it takes over. The active tab is kept if it is another. */
  const showTab = useCallback(
    (id: number | null, t: TabState) => {
      const takeOver = id === null && flowName === null && !viewing && live.current.nodes.length === 0 && !dirty;
      const target = id ?? (takeOver ? activeId : nextTabId.current++);
      if (target !== activeId) stash.current.set(activeId, captureTab());
      stash.current.delete(target);
      setTabIds((ids) => (ids.includes(target) ? ids : [...ids, target]));
      applyTab(t);
      setActiveId(target);
      setPage("canvas");
    },
    [flowName, viewing, dirty, activeId, captureTab, applyTab, setPage],
  );

  const switchTab = useCallback(
    (id: number) => {
      const t = stash.current.get(id);
      if (id === activeId || !t) return setPage("canvas");
      showTab(id, t);
    },
    [activeId, showTab, setPage],
  );

  /** The tab `name` is open in, if any. */
  const tabOf = useCallback(
    (name: string): number | null => {
      if (flowName === name) return activeId;
      for (const [id, t] of stash.current) if (t.meta.name === name) return id;
      return null;
    },
    [flowName, activeId],
  );

  const closeTab = useCallback(
    async (id: number) => {
      const t = id === activeId ? captureTab() : stash.current.get(id);
      if (!t) return;
      if (t.dirty && !(await dialog.confirm(`Close "${t.meta.name ?? "untitled"}" and discard its unsaved changes?`, "Discard"))) return;
      if (id !== activeId) {
        stash.current.delete(id);
        setTabIds((ids) => ids.filter((x) => x !== id));
        return;
      }
      const next = nextAfterClose(tabIds, id);
      if (next === null) {
        // The last tab closes into a fresh draft.
        applyTab(freshTab(null));
        return;
      }
      const nt = stash.current.get(next) ?? freshTab(null);
      stash.current.delete(next);
      setTabIds((ids) => ids.filter((x) => x !== id));
      applyTab(nt);
      setActiveId(next);
    },
    [activeId, captureTab, dialog, tabIds, applyTab],
  );

  // Leaving the page asks first while any tab has unsaved changes.
  const anyDirty = dirty || [...stash.current.values()].some((t) => t.dirty);
  useEffect(() => {
    if (!anyDirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [anyDirty]);

  // A flow document as a tab: its composites put back into the workspace first, its node-id
  // counter past every id it holds. `saved`: it is the saved flow of that name (else a draft,
  // unsaved until saved).
  const tabFromDocument = useCallback(
    (name: string | null, doc: FlowDocument, saved: boolean): TabState => {
      const ns = doc.nodes as PrimitiveNodeType[];
      const es = doc.edges as Edge[];
      // A preimage is re-hashed on insert, so a tampered entry can only add a unit nobody
      // points at, never stand in for one.
      const failed = restoreDocument(doc, ev);
      let msg = failed ? `some saved units could not be restored: ${failed}` : "";
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
        msg = `${missing.length} grouped node(s) can't be resolved (saved before composites were stored) — rebuild them, then save`;
      }
      const server: FlowSettings = {
        port: doc.server?.port ?? DEFAULT_SETTINGS.port,
        interval: doc.server?.interval ?? DEFAULT_SETTINGS.interval,
        ...(doc.server?.requirements ? { requirements: doc.server.requirements } : {}),
      };
      // A loaded flow's node ids carry the counters they were minted with: the next id must
      // start past all of them, or a drop would mint a colliding one.
      let max = -1;
      for (const n of ns) for (const m of n.id.matchAll(/\d+/g)) max = Math.max(max, Number(m[0]));
      return {
        ...blankTab(name),
        meta: { ...blankMeta(name), saved, settings: server, savedKey: saved ? savedKeyOf(ns, es, server) : "", saveMsg: msg },
        nodes: ns,
        edges: es,
        viewport: doc.viewport,
        history: newHistory(snapshotOf(ns, es)),
        idCounter: max + 1,
      };
    },
    [ev],
  );

  // "New" from the menu: a clean canvas under the prompted name, in a tab of its own. `nonce`
  // makes a second New with the same name still fire.
  useEffect(() => {
    if (!pendingNew) return;
    showTab(null, freshTab(pendingNew.name));
    // Only a new request opens a tab, not the callbacks changing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingNew]);

  const onSave = useCallback(async () => {
    let name = flowName;
    if (!name) {
      name = await dialog.prompt("Flow name (letters, digits, - and _ only):");
      if (!name) return;
      setFlowName(name);
    }
    let carried: ReturnType<typeof documentUnits>;
    try {
      carried = documentUnits(nodes.flatMap((n) => (n.data.body.kind === "Composite" ? [n.data.body.hash] : [])), ev);
    } catch (e) {
      setSaveMsg(`not saved: ${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    const doc: FlowDocument = {
      nodes: nodes as unknown[],
      edges: edges as unknown[],
      viewport: getViewport(),
      ...carried,
      server: settings,
    };
    const key = savedKeyOf(nodes, edges, settings);
    saveFlow(name, doc)
      .then((entry) => {
        setSavedKey(key);
        setSavedOnServer(true);
        setSaveMsg(`saved ${new Date(entry.saved_at_epoch_seconds * 1000).toLocaleTimeString()}`);
      })
      .catch((e: unknown) => setSaveMsg(e instanceof Error ? e.message : String(e)));
  }, [flowName, nodes, edges, getViewport, ev, settings, dialog]);

  // Open a saved flow: in its tab when it is open already (`reload`: replace that tab's canvas
  // with the saved copy, asking first if it has unsaved changes), else in a new tab.
  const onOpenFlow = useCallback(
    async (rec: FlowRecord, reload = false) => {
      const open = tabOf(rec.name);
      if (open !== null && !reload) {
        switchTab(open);
        return true;
      }
      const current = open === null ? null : open === activeId ? captureTab() : stash.current.get(open);
      if (current?.dirty && !(await dialog.confirm(`Discard your unsaved changes to "${rec.name}" and reload the saved copy?`, "Reload saved"))) {
        return false;
      }
      showTab(open, tabFromDocument(rec.name, rec.document, true));
      return true;
    },
    [tabOf, switchTab, activeId, captureTab, dialog, showTab, tabFromDocument],
  );

  // What a node does (NodeDocs.tsx): core's text for a primitive, the template's, or a holon's
  // said from what it is, with how many nodes it holds.
  const describeNode = useMemo(
    () =>
      nodeDescriber(docs, (hash) => {
        try {
          const u = ev.get_unit(hash) as WUnit;
          return u.body.kind === "Composite" ? u.body.children.length : null;
        } catch {
          return null;
        }
      }),
    [docs, ev],
  );

  const byName = useMemo(() => {
    const m = new Map<string, WUnit>();
    for (const u of catalog) {
      const n = primitiveName(u);
      if (n) m.set(n, u);
    }
    return m;
  }, [catalog]);

  // A sample (samples.ts) opens as an unsaved copy under its own name, in a tab of its own:
  // saving it keeps it. A tab of that name already open is replaced only when confirmed.
  const onOpenSample = useCallback(
    async (sample: Sample) => {
      const open = tabOf(sample.name);
      if (open !== null && !(await dialog.confirm(`"${sample.name}" is open in a tab. Replace it with the sample? Its unsaved changes are lost.`, "Replace"))) {
        return;
      }
      showTab(open, tabFromDocument(sample.name, buildSample(sample, byName, variadic, ev), false));
    },
    [tabOf, dialog, showTab, tabFromDocument, byName, variadic, ev],
  );

  const onDragOver = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
  }, []);

  const onDrop = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      e.preventDefault();
      const name = e.dataTransfer.getData(DND_MIME);
      if (viewing) return;
      // A saved flow, placed as one node (canvasOps.flowAsNode).
      if (name.startsWith(FLOW_PREFIX)) {
        const flow = name.slice(FLOW_PREFIX.length);
        const position = screenToFlowPosition({ x: e.clientX, y: e.clientY });
        getFlow(flow)
          .then((rec) => {
            const placed = flowAsNode(flow, rec.document, ev, byName, `${flow}-${idRef.current++}`, position, rec.saved_at_epoch_seconds);
            if (placed.ok) {
              setDeployError("");
              setNodes((ns) => ns.concat(placed.node));
            } else setDeployError(placed.message);
          })
          .catch((err: unknown) => setDeployError(err instanceof Error ? err.message : String(err)));
        return;
      }
      const node = nodeFromPalette(name, byName, variadic, `${name}-${idRef.current++}`, screenToFlowPosition({ x: e.clientX, y: e.clientY }));
      if (node) setNodes((ns) => ns.concat(node));
    },
    [byName, variadic, screenToFlowPosition, setNodes, viewing, ev],
  );

  // The saved flows the palette offers as nodes, and when each was saved (a holon placed from an
  // older save offers an update): read when the canvas shows and after a save.
  const [savedFlowTimes, setSavedFlowTimes] = useState<Map<string, number>>(new Map());
  useEffect(() => {
    if (page !== "canvas") return;
    listFlows()
      .then((fs) => setSavedFlowTimes(new Map(fs.map((f) => [f.name, f.saved_at_epoch_seconds]))))
      .catch(() => setSavedFlowTimes(new Map()));
  }, [page, savedKey]);
  const savedFlows = useMemo(() => [...savedFlowTimes.keys()], [savedFlowTimes]);

  // Place the newest saved version of a holon over the one placed from an older save: same place,
  // and its wires kept to the ports the new version still has.
  const updateHolon = useCallback(
    (id: string) => {
      const old = live.current.nodes.find((n) => n.id === id);
      const from = old?.data.source;
      if (!old || !from) return;
      getFlow(from.flow)
        .then((rec) => {
          const placed = flowAsNode(from.flow, rec.document, ev, byName, id, old.position, rec.saved_at_epoch_seconds);
          if (!placed.ok) return setDeployError(placed.message);
          const ins = new Set(placed.node.data.inputs.map((p) => p.name));
          const outs = new Set(placed.node.data.outputs.map((p) => p.name));
          const kept = (e: Edge) => (e.target !== id || ins.has(e.targetHandle ?? "")) && (e.source !== id || outs.has(e.sourceHandle ?? ""));
          const dropped = live.current.edges.filter((e) => !kept(e)).length;
          setNodes((ns) => ns.map((n) => (n.id === id ? { ...placed.node, selected: n.selected } : n)));
          setEdges((es) => es.filter(kept));
          setSaveMsg(`updated "${from.flow}" to its save of ${new Date(rec.saved_at_epoch_seconds * 1000).toLocaleString()}${dropped ? `; ${dropped} wire(s) to ports it no longer has removed` : ""}`);
        })
        .catch((e: unknown) => setDeployError(e instanceof Error ? e.message : String(e)));
    },
    [ev, byName, setNodes, setEdges, setSaveMsg],
  );


  // A group of canvas nodes as core children and wires, and the canvas node behind each child of
  // a frozen unit (canvasOps.ts).
  const groupUnits = useCallback((group: PrimitiveNodeType[], es: Edge[]) => groupOf(group, es, ev, byName), [ev, byName]);
  const traceOf = useCallback((unit: WUnit, group: PrimitiveNodeType[]) => traceOfUnit(unit, group, ev), [ev]);

  // A saved flow's manifest (the flows page), derived from its document as deploying it would:
  // its unit's capabilities from core, plus what its deploy settings ask beyond the defaults.
  const manifestOfDocument = useCallback(
    (doc: FlowDocument) => {
      restoreDocument(doc, ev);
      const hash = flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, byName, doc.server?.port ?? DEFAULT_SETTINGS.port);
      const caps = (ev.capability_names(hash) as string[]);
      return { items: manifestItems(caps, doc.server?.requirements), optional: doc.server?.requirements?.optional ?? [], trigger: manifestTrigger(caps) };
    },
    [ev, byName],
  );

  // The server this flow declares, derived from the canvas (canvasOps.deriveServer). Recomputed
  // when the canvas's structure changes, not on moves.
  const derived = useMemo(() => {
    const { nodes: ns, edges: es } = live.current;
    return deriveServer(ns, es, ev, byName, settings.port);
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, settings.port]);

  // What the canvas's flow would ask for, by consent name (empty while it doesn't derive): the
  // deploy settings offer each one but its trigger as something it can run without.
  const flowCapabilities = useMemo(() => {
    try {
      const { nodes: ns, edges: es } = live.current;
      const hash = flowUnitHash(ns, es, ev, byName, settings.port);
      return ((ev.capability_names(hash) as string[])).sort();
    } catch {
      return [];
    }
    // live.current is read on purpose, as `derived` above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, settings.port]);

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
      if (viewing) return;
      const srcPort = getNode(c.source)?.data.outputs.find((p) => p.name === c.sourceHandle);
      const srcTy = srcPort?.ty ?? null;
      const edge = { ...c, style: { stroke: outputColor(srcTy), strokeWidth: 1.6 } };
      setEdges((es) => addEdge(edge, es));
    },
    [getNode, setEdges, viewing],
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
  // in-browser wasm workspace. Deploying talks to holon-server, over plain HTTP, localhost-only.
  const deploymentTable = useDeployments(page, setDeployError);
  const { deployments, listError, refresh: refreshDeployments } = deploymentTable;
  const [deployBusy, setDeployBusy] = useState(false);

  // ── Deploy: one action for the whole flow, named after it (DeployBar in the header) ──────
  // A flow with routes deploys as its derived server; any other flow as the whole canvas
  // projected into one unit (a clock or ingress flow). Before anything is sent, the person sees
  // exactly what it may touch (the server's consent gate checks the same list), in the dialog.
  const deployedEntry = flowName ? deployments.find((d) => d.slug === flowName) : undefined;
  const outOfDate = deployedEntry && derived.serverHash ? deployedEntry.unit !== derived.serverHash : null;
  // The secrets the flow reads, by name: what ⚙ lets you set. From its derived manifest, so a
  // secret inside a placed holon counts; plus its own `secret` nodes, for while it doesn't derive.
  const secretNames = useMemo(
    () =>
      [
        ...new Set([
          ...flowCapabilities.flatMap((c) => {
            const m = /^SecretRead\("(.*)"\)$/.exec(c);
            return m ? [m[1]] : [];
          }),
          ...nodes.flatMap((n) =>
            n.data.name === "secret" && n.data.body.kind === "Primitive"
              ? n.data.params.flatMap((p) => (p.name === "name" && p.kind === "Frozen" ? [p.value] : []))
              : [],
          ),
        ]),
      ].sort(),
    [nodes, flowCapabilities],
  );

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
    let trace = derived.trace;
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
        const whole = ev.freeze_encapsulation(children, wedges, spec) as WUnit;
        hash = ev.insert(buildPreimage(whole));
        trace = { "*": traceOf(whole, ns) };
      }
    } catch (err) {
      setDeployError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      return;
    }
    const caps = (ev.capability_names(hash) as string[]);
    let interval: number | undefined;
    if (!isServer && caps.includes("Clock")) {
      interval = Number(settings.interval);
      if (!Number.isInteger(interval) || interval < 1) {
        setDeployError("this flow reads the clock: set how often it fires under ⚙ (whole seconds, at least 1)");
        return;
      }
    }
    // The host decides what needs approval (its policy, and what this deployment was already
    // granted): ask it, then show asked against granted, and acknowledge exactly what it asks.
    const existing = deployments.some((d) => d.slug === name);
    const requirements = settings.requirements;
    let ask: string[];
    try {
      await stageClosure(ev, hash, new Set());
      const preview = await previewDeploy(name, hash, requirements);
      if (preview.refused) {
        setDeployError(`this host can't grant that: ${preview.refused}`);
        return;
      }
      ask = preview.needs_approval;
      // Each item says whether the flow can run without it (deploy settings' optional ones).
      const optional = requirements?.optional ?? [];
      const said = (i: string) => `• ${describeItem(i)}${optional.includes(i) ? " (optional: it runs without it)" : ""}`;
      const lines = [`${existing ? "Update" : "Deploy"} "${name}"?`];
      if (ask.length > 0) lines.push("", "It needs your approval to:", ...ask.map(said));
      if (preview.already_granted.length > 0) {
        lines.push("", "Already granted:", ...preview.already_granted.map(said));
      }
      if (preview.missing_secrets.length > 0) {
        lines.push(
          "",
          `Not set yet: ${preview.missing_secrets.join(", ")} (set under ⚙ → secrets; until then those nodes stay empty)`,
        );
      }
      if (existing) lines.push("", "Its state and sessions are kept.");
      const action = ask.length > 0 ? "Approve and deploy" : existing ? "Update" : "Deploy";
      if (!(await dialog.confirm(lines.join("\n"), action))) return;
    } catch (err) {
      setDeployError(err instanceof Error ? err.message : String(err));
      return;
    }
    setDeployBusy(true);
    try {
      const { nodes: ns, edges: es } = live.current;
      const carried = documentUnits(ns.flatMap((n) => (n.data.body.kind === "Composite" ? [n.data.body.hash] : [])), ev);
      const document: FlowDocument = { nodes: ns, edges: es, viewport: getViewport(), ...carried, server: settings, trace };
      await deployUnit(name, hash, ask, interval, document, requirements);
      refreshDeployments();
    } catch (err) {
      setDeployError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeployBusy(false);
    }
  }, [flowName, dialog, derived, settings, groupUnits, traceOf, ev, deployments, refreshDeployments, getViewport]);

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

  // ── View mode ──────────────────────────────────────────────────────────────────────────
  // A deployment, read only, with its runs: in its flow's tab (that tab's edit copy set aside
  // until edit mode), or a tab of its own.
  const onView = useCallback(
    async (slug: string) => {
      let doc: FlowDocument;
      try {
        doc = await getDeployedDocument(slug);
      } catch (e) {
        setDeployError(e instanceof Error ? e.message : String(e));
        return;
      }
      // A unit that won't decode just stays unresolvable, like on Open.
      restoreDocument(doc, ev);
      const open = tabOf(slug);
      const base = open === null ? blankTab(slug) : open === activeId ? captureTab() : (stash.current.get(open) ?? blankTab(slug));
      const editCopy = base.meta.viewing
        ? base.beforeView
        : open === null
          ? null
          : { nodes: base.nodes, edges: base.edges, viewport: base.viewport, history: base.history, dirty: base.dirty };
      showTab(open, {
        ...base,
        beforeView: editCopy,
        nodes: doc.nodes as PrimitiveNodeType[],
        edges: doc.edges as Edge[],
        viewport: doc.viewport,
        history: newHistory(snapshotOf([], [])),
        viewDoc: doc,
        meta: {
          ...base.meta,
          viewing: slug,
          viewUnit: deployments.find((d) => d.slug === slug)?.unit ?? null,
          viewTrace: doc.trace,
          selectedRun: null,
          step: null,
          inspectNode: null,
        },
      });
    },
    [ev, tabOf, activeId, captureTab, showTab, deployments],
  );

  // View mode: the run replayed on its version's canvas, stepping, flow state, and the panes
  // (useViewMode.tsx).
  const view = useViewMode({
    flow,
    setSelectedRun,
    setStep,
    setInspectNode,
    setViewTrace,
    setViewUnit,
    nodes,
    edges,
    live,
    canvasKey,
    setNodes,
    setEdges,
    restoring,
    viewDoc,
    ev,
    byName,
    deployments,
    fitView,
  });

  // Edit mode: the flow as edited, put back as it was left; for a tab opened to view, the saved
  // flow of that name, else a draft of what is deployed.
  const onEditMode = useCallback(async () => {
    const before = beforeView.current;
    if (!viewing) return;
    if (before) {
      beforeView.current = null;
      viewDoc.current = null;
      setViewing(null);
      setViewTrace(undefined);
      setInspectNode(null);
      setSelectedRun(null);
      setStep(null);
      history.current = before.history;
      restoring.current = true;
      setNodes(before.nodes);
      setEdges(before.edges);
      setViewport(before.viewport);
      bump();
      return;
    }
    const name = flowName;
    // What is deployed now, not an earlier version a run may be shown on.
    const doc = viewing ? await getDeployedDocument(viewing).catch(() => viewDoc.current) : viewDoc.current;
    let t: TabState | null = null;
    if (name) {
      try {
        t = tabFromDocument(name, (await getFlow(name)).document, true);
      } catch {
        // not saved: start from what is deployed
      }
    }
    if (!t && doc) t = tabFromDocument(name, doc, false);
    if (t) showTab(activeId, t);
  }, [viewing, flowName, history, restoring, setNodes, setEdges, setViewport, bump, tabFromDocument, showTab, activeId]);

  const onMode = useCallback(
    (m: Mode) => {
      if (m === "edit") onEditMode();
      else if (flowName) onView(flowName);
    },
    [onEditMode, onView, flowName],
  );

  const onProject = useCallback(() => {
    const result = projectCanvas(nodes, edges, ev, byName, canvasKey);
    if (result.ok) {
      setProjection(result.projection);
      setProjError("");
      setErrorNodeIds(new Set());
    } else {
      setProjection(null);
      setProjError(result.message);
      setErrorNodeIds(new Set(result.nodeIds));
    }
  }, [nodes, edges, ev, canvasKey, byName]);

  // Group the current selection into one placed composite node (P4.3.1). React Flow's own
  // selection (shift-click / drag-box) is already live and already used for delete.
  const onGroup = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length < 2) return; // mirrors the button's own disabled guard, defensively
    const result = groupSelection(selected, edges, ev, byName, () => idRef.current++);
    if (result.ok) {
      const selectedIds = new Set(selected.map((n) => n.id));
      setNodes((ns) => [...ns.filter((n) => !selectedIds.has(n.id)), result.node]);
      setEdges(result.edges);
      setProjError("");
      setErrorNodeIds(new Set());
    } else {
      setProjError(result.message);
      setErrorNodeIds(new Set(result.nodeIds));
    }
  }, [nodes, edges, ev, setNodes, setEdges, byName]);

  // Expand the one selected composite back into its children, one level.
  // What each composite has been called on this editor's canvases, by hash: a unit keeps no
  // names, so a composite found inside another (expanding or ungrouping it) takes its name from
  // here, e.g. a "slugify" grouped into something bigger stays "slugify" inside it.
  // Saved and deployed with the flow, and learnt back on open (docUnits.ts).
  useEffect(() => {
    for (const n of nodes) if (n.data.body.kind === "Composite") rememberName(n.data.body.hash, n.data.name);
  }, [nodes]);

  // Ungroup a composite: replace it with the nodes inside it, here (canvasOps.expandComposite).
  const ungroup = useCallback((id: string) => {
    const target = live.current.nodes.find((n) => n.id === id);
    if (!target || target.data.body.kind !== "Composite") return;
    const result = expandComposite(target, live.current.edges, ev, byName, variadic, () => idRef.current++, nameOf);
    if (result.ok) {
      setNodes((ns) => [...ns.filter((n) => n.id !== target.id), ...result.nodes]);
      setEdges(result.edges);
      setProjError("");
      setErrorNodeIds(new Set());
    } else {
      setProjError(result.message);
    }
  }, [ev, byName, variadic, setNodes, setEdges, nameOf]);

  // Look inside a composite: its inner flow on a canvas of its own, over this one, grown from
  // where the node sits (CompositeOverlay.tsx). In view mode, with a run selected, its nodes show
  // what they produced in the run (the log keeps one level inside each node).
  const [overlay, setOverlay] = useState<{ frames: OverlayFrame[]; from: { left: number; top: number; width: number; height: number } } | null>(null);
  // Its nodes are read only in view mode, and changed like the flow's in edit mode.
  const editableInside = useCallback((ns: FlowNode[]) => (viewing ? ns : ns.map((n) => (n.type === "primitive" ? { ...n, data: { ...n.data, readOnly: false } } : n))), [viewing]);
  const inside = useCallback(
    (target: PrimitiveNodeType, around: FlowNode[], wires: Edge[]): OverlayFrame | string => {
      const opened = innerCanvas(target, around.filter((n) => n.type === "primitive") as PrimitiveNodeType[], wires, ev, byName, variadic, nameOf);
      return opened.ok ? { title: target.data.name, nodeId: target.id, nodes: editableInside(opened.nodes), edges: opened.edges, dirty: false } : opened.message;
    },
    [ev, byName, variadic, nameOf, editableInside],
  );
  // What the overlay's levels need to change: freeze a level, place a palette entry, ungroup.
  const overlayOps = useMemo<OverlayOps>(
    () => ({
      inside,
      freeze: (canvas) => freezeInner(canvas, ev, byName),
      paletteNode: (name, position) => nodeFromPalette(name, byName, variadic, `${name}-${idRef.current++}`, position),
      ungroupIn: (canvas, id) => {
        const target = canvas.nodes.find((n) => n.id === id) as PrimitiveNodeType | undefined;
        if (!target || target.data.body.kind !== "Composite") return "not a composite";
        const result = expandComposite(target, canvas.edges, ev, byName, variadic, () => idRef.current++, nameOf);
        return result.ok ? { nodes: [...canvas.nodes.filter((n) => n.id !== id), ...result.nodes], edges: result.edges } : result.message;
      },
    }),
    [inside, ev, byName, variadic, nameOf],
  );
  // Apply: the composite in the flow replaced by its new version; wires follow renamed ports,
  // and wires to ports it no longer has are removed, saying so. Undo takes it back.
  const applyComposite = useCallback(
    (id: string, frozen: FrozenOk, renames: Renames) => {
      const done = replaceComposite(live.current.nodes, live.current.edges, id, frozen, renames);
      setNodes(done.nodes);
      setEdges(done.edges);
      const name = live.current.nodes.find((n) => n.id === id)?.data.name ?? "composite";
      setSaveMsg(`"${name}" is a new version${done.dropped.length ? `; ${done.dropped.length} wire(s) to ports it no longer has removed` : ""}`);
    },
    [setNodes, setEdges, setSaveMsg],
  );
  const expandNode = useCallback(
    (id: string) => {
      const target = live.current.nodes.find((n) => n.id === id);
      if (!target) return;
      const opened = innerCanvas(target, live.current.nodes, live.current.edges, ev, byName, variadic, nameOf);
      if (!opened.ok) return setProjError(opened.message);
      let frameNodes = opened.nodes;
      if (viewing && flow.selectedRun) {
        const steps = innerSteps(flow.selectedRun, flow.viewTrace, id);
        frameNodes = frameNodes.map((n) => {
          const step = steps.find((st) => opened.childIds[st.child] === n.id);
          return step ? { ...n, data: { ...n.data, trace: { outputs: step.outputs ?? [], state: "done", showValues: true } } } : n;
        });
      }
      const box = document.querySelector(".hl-canvas__flow")?.getBoundingClientRect();
      const at = document.querySelector(`.hl-canvas__flow .react-flow__node[data-id="${CSS.escape(id)}"]`)?.getBoundingClientRect();
      const from = box && at ? { left: at.left - box.left, top: at.top - box.top, width: at.width, height: at.height } : { left: 0, top: 0, width: 200, height: 100 };
      setOverlay({ frames: [{ title: target.data.name, nodeId: id, nodes: editableInside(frameNodes), edges: opened.edges, dirty: false }], from });
    },
    [ev, byName, variadic, viewing, flow.selectedRun, flow.viewTrace, nameOf, editableInside],
  );
  // Another tab, or the other mode, closes it.
  useEffect(() => setOverlay(null), [activeId, viewing]);
  const nodeActions = useMemo(() => ({ updateHolon, expandComposite: expandNode, ungroup }), [updateHolon, expandNode, ungroup]);

  const onExpand = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length !== 1) return; // mirrors the button's own disabled guard, defensively
    const target = selected[0];
    if (target.data.body.kind !== "Composite") return;
    ungroup(target.id);
  }, [nodes, ungroup]);

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

  // Exclude one currently-exposed output and re-freeze (canvasOps.excludeProjectedOutput).
  const excludeOutput = useCallback(
    (index: number) => setProjection((p) => (p ? excludeProjectedOutput(p, index, ev) : p)),
    [ev],
  );

  // Purely a rendering decoration for React Flow — never touches the underlying `nodes` state
  // that onProject/onDrop read and write, so flagging a node as errored has zero effect on
  // hashing or identity (the same "cosmetic vs. structural" line I5 already draws elsewhere).
  // In view mode the run decorates them instead (useViewMode).
  const decoratedNodes = useMemo(() => {
    if (viewing) return view.viewNodes;
    // A holon placed from a saved flow that has been saved since offers its update.
    const outdated = (n: PrimitiveNodeType) => {
      const src = n.data.source;
      const latest = src ? savedFlowTimes.get(src.flow) : undefined;
      return src !== undefined && latest !== undefined && latest > src.savedAt;
    };
    if (errorNodeIds.size === 0 && !nodes.some(outdated)) return nodes;
    return nodes.map((n) => {
      const flags = { ...(errorNodeIds.has(n.id) ? { error: true } : {}), ...(outdated(n) ? { updateAvailable: true } : {}) };
      return Object.keys(flags).length > 0 ? { ...n, data: { ...n.data, ...flags } } : n;
    });
  }, [nodes, errorNodeIds, viewing, view.viewNodes, savedFlowTimes]);

  const decoratedEdges = viewing ? view.viewEdges : edges;

  // How many placed nodes are currently selected — React Flow's own selection state
  // (shift-click / drag-box), already live for delete, now also gating the "group" button
  // (P4.3.1). Cheap enough over a canvas-sized node list not to need memoizing.
  const selectedCount = nodes.filter((n) => n.selected).length;
  // "Expand" is the dual of "group": only meaningful for exactly one selected composite node —
  // mirrors group's own selectedCount<2 guard rather than a new per-node interaction
  // PrimitiveNode.tsx doesn't otherwise have for whole-node operations.
  const canExpand = selectedCount === 1 && nodes.some((n) => n.selected && n.data.body.kind === "Composite");

  // The tab strip: each open flow, what it is, and its mode.
  const tabList = tabIds.map((id) => {
    const t = id === activeId ? { meta: flow, dirty } : (stash.current.get(id) ?? blankTab(null));
    return {
      id,
      name: t.meta.name,
      kind: tabKind(t.meta, (name) => deployments.some((d) => d.slug === name)),
      dirty: t.dirty,
      mode: (t.meta.viewing ? "view" : "edit") as Mode,
    };
  });

  return (
    <NodeDocs.Provider value={describeNode}>
    <NodeActions.Provider value={nodeActions}>
    {/* Edit mode places nodes from the palette; view mode is read only, so it has none. */}
    {!viewing && page === "canvas" && (
      // A flow isn't offered inside itself.
      <Palette catalog={catalog} templates={TEMPLATES} flows={savedFlows.filter((f) => f !== flowName)} docs={docs} />
    )}
    <div className="hl-main">
    <FlowTabs tabs={tabList} active={activeId} onSelect={switchTab} onClose={closeTab} />
    <div className={viewing ? "hl-canvas hl-canvas--viewing" : "hl-canvas"} onDrop={onDrop} onDragOver={onDragOver}>
      <div className="hl-canvas__flow">
        <ReactFlow<PrimitiveNodeType>
          nodes={decoratedNodes}
          onNodeClick={view.onNodeClick}
          onEdgeMouseEnter={viewing ? (_, edge) => view.setHoveredWire(edge.id) : undefined}
          onEdgeMouseLeave={viewing ? () => view.setHoveredWire(null) : undefined}
          onNodesChange={onNodesChange}
          edges={decoratedEdges}
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
          deleteKeyCode={viewing ? null : ["Backspace", "Delete"]}
          nodesDraggable={!viewing}
          nodesConnectable={!viewing}
        >
          <Background variant={BackgroundVariant.Lines} gap={26} color="#1b2a45" />
          <Controls showInteractive={false} />
          <Panel position="top-left">
            <FlowToolbar
              histVersion={histVersion}
              mode={viewing ? "view" : "edit"}
              canView={deployedEntry !== undefined}
              onMode={onMode}
              showValues={view.showValues}
              onToggleValues={view.toggleValues}
              flowName={flowName}
              dirty={dirty}
              onSave={onSave}
              canUndo={history.current.past.length > 0}
              canRedo={history.current.future.length > 0}
              onUndo={undo}
              onRedo={redo}
              deployBar={
                <DeployBar
                  entry={deployedEntry}
                  outOfDate={outOfDate}
                  busy={deployBusy}
                  settings={settings}
                  setSettings={setSettings}
                  onDeploy={onDeployFlow}
                  onAction={onFlowAction}
                  slug={flowName}
                  secretNames={secretNames}
                  capabilities={flowCapabilities}
                />
              }
              saveMsg={saveMsg}
              error={page === "canvas" ? deployError || listError : ""}
            />
          </Panel>
          {!viewing && (
            <Panel position="top-right">
              <ProjectionPanel
                nodeCount={nodes.length}
                selectedCount={selectedCount}
                canExpand={canExpand}
                onProject={onProject}
                onGroup={onGroup}
                onExpand={onExpand}
                onInspect={onInspect}
                projection={projection}
                onBoundaryChange={setBoundaryValue}
                onExcludeOutput={excludeOutput}
                error={projError}
              />
            </Panel>
          )}
          {!viewing && (
            <Panel position="bottom-left" style={{ marginLeft: 56 }}>
              <ServerPanel
                key={flowName ?? ""}
                ev={ev}
                endpoints={derived.endpoints}
                errors={derived.errors}
                liveSlug={deployedEntry && deployedEntry.port !== null ? (flowName ?? "") : ""}
              />
            </Panel>
          )}
          {inspecting && (
            <Panel position="bottom-right">
              <InspectPanel hash={inspecting} ev={ev} onClose={() => setInspecting(null)} />
            </Panel>
          )}
        </ReactFlow>
        {nodes.length === 0 && page === "canvas" && (
          <div className="hl-canvas__empty">drag a primitive here to place it</div>
        )}
        {overlay && (
          <CompositeOverlay
            root={flowName ?? "untitled"}
            frames={overlay.frames}
            from={overlay.from}
            editable={!viewing}
            ops={overlayOps}
            onFrames={(update) => setOverlay((o) => (o ? { ...o, frames: update(o.frames) } : o))}
            onApply={applyComposite}
            onClose={() => setOverlay(null)}
          />
        )}
      </div>
      <ViewModePanes view={view} flow={flow} nodes={nodes} />
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
          {page === "flows" && (
            <FlowsPage
              openTabs={tabList}
              onOpen={onOpenFlow}
              onSwitch={switchTab}
              samples={SAMPLES}
              onOpenSample={onOpenSample}
              manifestOf={manifestOfDocument}
            />
          )}
          {page === "deployed" && (
            <DeployedPage
              deployments={deployments}
              error={deployError || listError}
              onAction={(slug, action, from) => changeDeployment(slug, action, from).then(() => refreshDeployments())}
              triggerInputs={deploymentTable.triggerInputs}
              onTriggerInputChange={deploymentTable.onTriggerInputChange}
              onTrigger={deploymentTable.onTrigger}
              triggerResults={deploymentTable.triggerResults}
              onDelete={deploymentTable.onDelete}
              onView={onView}
              onChanged={refreshDeployments}
            />
          )}
        </div>
      )}
    </div>
    </div>
    </NodeActions.Provider>
    </NodeDocs.Provider>
  );
}

function Editor() {
  const dialog = useDialog();
  const [catalog, setCatalog] = useState<WUnit[] | null>(null);
  // What each primitive does, from core: on each node, the palette and the component tab.
  const [docs, setDocs] = useState<Map<string, string>>(new Map());
  const [variadic, setVariadic] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string>("");
  const [page, setPage] = useState<AppPage>("canvas");
  // "New" carries the prompted name down to the canvas, which opens it in a tab of its own;
  // `nonce` makes a second New with the same name still take effect.
  const [pendingNew, setPendingNew] = useState<{ name: string; nonce: number } | null>(null);

  useEffect(() => {
    (async () => {
      await ensureReady();
      setDocs(primitiveDocs());
      setCatalog(primitiveCatalog());
      setVariadic(variadicPrimitives());
    })().catch((e: unknown) => setError(String(e)));
  }, []);

  const onNew = async () => {
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
          <AccessButton style={menuBtn(false)} />
        </nav>
        <span className="hl-header__count">
          {catalog ? `${catalog.length} genesis atoms` : "loading core…"}
        </span>
      </header>

      {error ? (
        <pre className="hl-error">{error}</pre>
      ) : (
        <div className="hl-body">
          <ReactFlowProvider>
            {catalog && (
              <Canvas
                catalog={catalog}
                variadic={variadic}
                page={page}
                setPage={setPage}
                pendingNew={pendingNew}
                docs={docs}
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