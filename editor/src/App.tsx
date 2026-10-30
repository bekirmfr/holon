// The authoring canvas: a palette (derived from core's catalog) on the left, a blueprint canvas on
// the right. Drag a primitive across to place a typed node. Nothing here re-asserts a primitive's
// shape; every placed node reads its ports from the catalog interface (I1). The canvas's own
// operations live in canvasOps.ts, its panels in CanvasPanels.tsx, the pages over it in Pages.tsx,
// and its undo history and deployment table in their hooks.

import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent } from "react";
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
  Evaluator,
  type WUnit,
  type WValue,
  manifestItems,
  manifestTrigger,
} from "./holon";
import { changeDeployment, getFlow, getPrefs, setPrefs, type FlowDocument, type FlowRecord } from "./server";
import { outputColor } from "./theme";
import { Palette, DND_MIME, FLOW_PREFIX } from "./Palette";
import { PrimitiveNode, type PrimitiveNodeType } from "./PrimitiveNode";
import { UnitTree } from "./UnitTree";
import { Inspector } from "./ViewPanes";
import { structuralKey } from "./flowDoc";
import { knownNames, rememberName, restoreDocument } from "./docUnits";
import { DialogProvider, useDialog } from "./Dialog";
import { ToolsPanel } from "./ServerPanel";
import { DeployBar, DeployPanel, DEFAULT_SETTINGS } from "./DeployBar";
import { templateNames } from "./templates";
import { primitiveName, unitFor } from "./canvasUnits";
import {
  deriveServer,
  excludeProjectedOutput,
  flowAsNode,
  nodeFromPalette,
  flowUnitHash,
  groupSelection,
  lighterTriggers,
  projectCanvas,
  takesTrigger,
  tieEdge,
  triggersOf,
  untiedSinks,
  type Projection,
} from "./canvasOps";
import { expandComposite } from "./composites";
import { FlowTabs, FlowToolbar, ProjectionPanel } from "./CanvasPanels";
import { DeployedPage, FlowsPage, type AppPage, type PageTab } from "./Pages";
import { useCanvasHistory } from "./useCanvasHistory";
import { useDeployments } from "./useDeployments";
import { buildSample, samples, type Sample } from "./samples";
import { blankTab, freshTab, metaSetter, savedKeyOf, sourceToSave, syncKeyOf, tabFromDocument as tabFromDoc, tabKind, type FlowMeta, type Mode, type TabState } from "./tabs";
import { NodeActions, NodeDocs, nodeDescriber } from "./NodeDocs";
import { AccountMenu, AuthGate, type Me } from "./AuthGate";
import { AdminPage } from "./AdminPage";
import { HomePage } from "./HomePage";
import { compileHolon, printHolon, runnableByHand, sourceLineOf } from "./holonText";
import { TextPane } from "./TextPane";
import { useTextPane } from "./useTextPane";
import { WelcomePage } from "./WelcomePage";
import type { ManualAt } from "./ManualPage";
import { defaultHome, readPrefs, type Prefs } from "./home";
import { inspectorRun, useViewMode, ViewModePanes } from "./useViewMode";
import { useCompositeOverlay } from "./useCompositeOverlay";
import { useSavedFlows } from "./useSavedFlows";
import { useDeployFlow } from "./useDeployFlow";
import { useTabs } from "./useTabs";
import { useModeSwitch } from "./useModeSwitch";
import { useMarketplace } from "./useMarketplace";
import { MARKET_PREFIX } from "./marketplace";

// The docs tab, with the manual in it: loaded when first opened, not with the editor.
const ManualPage = lazy(() => import("./ManualPage").then((m) => ({ default: m.ManualPage })));

const nodeTypes = { primitive: PrimitiveNode } as unknown as NodeTypes;

function Canvas({
  catalog,
  variadic,
  page,
  setPage,
  pendingNew,
  docs,
  me,
  start,
}: {
  catalog: WUnit[];
  variadic: Set<string>;
  /** Who is signed in (AuthGate.tsx). */
  me: Me;
  /** The page tabs (home, welcome, flows, deployed, admin), and what home and the welcome keep
   *  with the account (home.ts). */
  start: {
    prefs: Prefs;
    onPrefs: (p: Prefs) => void;
    pages: PageTab[];
    openPage: (p: PageTab) => void;
    closePage: (p: PageTab) => void;
    onNew: () => void;
  };
  page: AppPage;
  setPage: (p: AppPage) => void;
  pendingNew: { nonce: number } | null;
  docs: Map<string, string>;
}) {
  const dialog = useDialog();
  const [nodes, setNodes, onNodesChange] = useNodesState<PrimitiveNodeType>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { screenToFlowPosition, getNode, getNodes, getEdges, getViewport, setViewport, fitView } =
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
  // Whether the Inspector is open or folded to a strip, remembered per browser.
  const [inspectorOpen, setInspectorOpen] = useState(() => {
    try {
      return localStorage.getItem("holon.inspector") !== "closed";
    } catch {
      return true;
    }
  });
  const onInspectorOpen = useCallback((open: boolean) => {
    setInspectorOpen(open);
    try {
      localStorage.setItem("holon.inspector", open ? "open" : "closed");
    } catch {
      // Not remembered, then.
    }
  }, []);
  const [paletteOpen, setPaletteOpen] = useState(() => {
    try { return localStorage.getItem("holon.palette") !== "closed"; } catch { return true; }
  });
  const onPaletteOpen = useCallback((open: boolean) => {
    setPaletteOpen(open);
    try { localStorage.setItem("holon.palette", open ? "open" : "closed"); } catch { /* ignore */ }
  }, []);
  const [deployPanelOpen, setDeployPanelOpen] = useState(() => {
    try { return localStorage.getItem("holon.deploy") !== "closed"; } catch { return false; }
  });
  const onDeployPanelOpen = useCallback((open: boolean) => {
    setDeployPanelOpen(open);
    try { localStorage.setItem("holon.deploy", open ? "open" : "closed"); } catch { /* ignore */ }
  }, []);
  const [toolsPanelOpen, setToolsPanelOpen] = useState(() => {
    try { return localStorage.getItem("holon.tools") !== "closed"; } catch { return true; }
  });
  const onToolsPanelOpen = useCallback((open: boolean) => {
    setToolsPanelOpen(open);
    try { localStorage.setItem("holon.tools", open ? "open" : "closed"); } catch { /* ignore */ }
  }, []);

  const canvasKey = useMemo(() => structuralKey(nodes, edges), [nodes, edges]);
  useEffect(() => {
    setProjection((p) => (p && p.basis !== canvasKey ? null : p));
  }, [canvasKey]);

  // The last deploy or deployment error, shown in the toolbar and on the deployed page.
  const [deployError, setDeployError] = useState("");
  // The deployment card asked for from the admin page's permissions, at its `n`th asking.
  const [focus, setFocus] = useState<{ slug: string; n: number } | null>(null);
  // Where the docs tab is in the manual (null: its index): kept here, so it is where it was when
  // the tab shows again.
  const [manualAt, setManualAt] = useState<ManualAt | null>(null);

  // New and Open start from a clean slate: no projection, error highlight or inspect panel
  // carried over from the previous flow.
  const resetProjectionView = useCallback(() => {
    setProjection(null);
    setProjError("");
    setErrorNodeIds(new Set());
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
  const [flow, setFlow] = useState<FlowMeta>(() => freshTab(null).meta);
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
  // Whether a composite is open over the canvas (useCompositeOverlay, below): its undo, not the
  // flow's, then answers.
  const overlayOpen = useRef(false);
  const { history, restoring, histVersion, bump, undo, redo } = useCanvasHistory(
    nodes,
    edges,
    setNodes,
    setEdges,
    () => viewing !== null || overlayOpen.current,
  );
  // The canvas as of this render, for callbacks that read it without depending on it.
  const live = useRef({ nodes, edges });
  live.current = { nodes, edges };
  // The text a save keeps with the flow (the pane's, as typed, while it stands for the canvas).
  const source = useMemo(() => sourceToSave(flow.text, syncKeyOf(nodes, edges, settings, flowName)), [flow.text, nodes, edges, settings, flowName]);
  const dirty = useMemo(
    // While viewing, what counts is whether the flow set aside had unsaved changes.
    () => (viewing ? (beforeView.current?.dirty ?? false) : savedKeyOf(nodes, edges, settings, source) !== savedKey),
    [nodes, edges, settings, source, savedKey, viewing],
  );

  // ── Tabs: one per open flow (useTabs.ts) ──────────────────────────────────────────────────
  const { tabIds, activeId, stash, captureTab, showTab, switchTab, tabOf, closeTab } = useTabs({
    flow,
    setFlow,
    live,
    dirty,
    getViewport,
    setViewport,
    history,
    restoring,
    bump,
    idRef,
    beforeView,
    viewDoc,
    setNodes,
    setEdges,
    resetProjectionView,
    showCanvas: () => setPage("canvas"),
    confirm: dialog.confirm,
  });
  const tabFromDocument = useCallback((name: string | null, doc: FlowDocument, saved: boolean) => tabFromDoc(name, doc, saved, ev), [ev]);
  // What the work area shows: the page asked for, or, with no flow open, home.
  const shown: AppPage = page === "canvas" && tabIds.length === 0 ? "home" : page;

  // "New" from the menu: a clean, untitled canvas in a tab of its own, named when it is first
  // saved (or deployed). `nonce` makes each New fire.
  useEffect(() => {
    if (!pendingNew) return;
    showTab(null, freshTab(null));
    // Only a new request opens a tab, not the callbacks changing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingNew]);

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

  // Saving it, the saved flows around it, and updating a holon placed from one (useSavedFlows.ts).
  const { savedFlowTimes, savedFlows, onSave, updateHolon } = useSavedFlows({
    refreshOn: page === "canvas" ? savedKey : null,
    flowName,
    setFlowName,
    nodes,
    edges,
    live,
    settings,
    source,
    getViewport,
    ev,
    dialog,
    setNodes,
    setEdges,
    setSavedKey,
    setSavedOnServer,
    setSaveMsg,
    setError: setDeployError,
  });
  // The marketplace: its listings in the palette, publishing from the flows page (useMarketplace.ts).
  const market = useMarketplace({ refreshOn: page === "canvas" || page === "flows" ? page : null, ev, dialog });

    // A flow other saved flows place as a node: they keep the version they placed, but say what
    // this save takes away from them should they update.
  // A sample (holon-author's samples.rs) opens as an unsaved copy under its own name, in a tab of its own:
  // saving it keeps it. A tab of that name already open is replaced only when confirmed.
  const onOpenSample = useCallback(
    async (sample: Sample) => {
      const open = tabOf(sample.name);
      if (open !== null && !(await dialog.confirm(`"${sample.name}" is open in a tab. Replace it with the sample? Its unsaved changes are lost.`, "Replace"))) {
        return;
      }
      showTab(open, tabFromDocument(sample.name, buildSample(sample, ev), false));
    },
    [tabOf, dialog, showTab, tabFromDocument, byName, variadic, ev],
  );
  // A flow's text (imported on the flows page, or a flow in the manual) opens as an unsaved copy
  // in a tab of its own; why it can't, else null.
  const openText = useCallback(
    (text: string): string | null => {
      try {
        const { name, doc } = compileHolon(text, ev);
        showTab(null, tabFromDocument(name, doc, false));
        return null;
      } catch (e) {
        return e instanceof Error ? e.message : String(e);
      }
    },
    [ev, showTab, tabFromDocument],
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
      // A holon from the marketplace, once its manifest is seen (useMarketplace.ts).
      if (name.startsWith(MARKET_PREFIX)) {
        const id = name.slice(MARKET_PREFIX.length);
        const position = screenToFlowPosition({ x: e.clientX, y: e.clientY });
        market.place(id, `market-${idRef.current++}`, position).then((placed) => {
          if (placed === null) return;
          if (typeof placed === "string") return setDeployError(placed);
          setDeployError("");
          setNodes((ns) => ns.concat(placed));
        });
        return;
      }
      // A saved flow, placed as one node (canvasOps.flowAsNode).
      if (name.startsWith(FLOW_PREFIX)) {
        const flow = name.slice(FLOW_PREFIX.length);
        const position = screenToFlowPosition({ x: e.clientX, y: e.clientY });
        getFlow(flow)
          .then((rec) => {
            const placed = flowAsNode(flow, rec.document, ev, `${flow}-${idRef.current++}`, position, rec.saved_at_epoch_seconds);
            if (placed.ok) {
              setDeployError("");
              setNodes((ns) => ns.concat(placed.node));
            } else setDeployError(placed.message);
          })
          .catch((err: unknown) => setDeployError(err instanceof Error ? err.message : String(err)));
        return;
      }
      const node = nodeFromPalette(name, byName, variadic, `${name}-${idRef.current++}`, screenToFlowPosition({ x: e.clientX, y: e.clientY }));
      if (!node) return;
      setNodes((ns) => ns.concat(node));
      // A sink placed on a canvas with one trigger is tied to it; with several, it is flagged.
      const triggers = triggersOf(getNodes());
      if (takesTrigger(node) && triggers.length === 1) setEdges((es) => es.concat(tieEdge(node.id, triggers[0])));
    },
    [byName, variadic, screenToFlowPosition, setNodes, setEdges, getNodes, viewing, ev, market],
  );

  // A saved flow's manifest (the flows page), derived from its document as deploying it would:
  // its unit's capabilities from core, plus what its deploy settings ask beyond the defaults.
  const manifestOfDocument = useCallback(
    (doc: FlowDocument) => {
      restoreDocument(doc, ev);
      const port = doc.server?.port ?? DEFAULT_SETTINGS.port;
      const hash = flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, port);
      const server = deriveServer(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, port);
      const scheduled = (doc.server?.interval ?? "").trim() !== "" && server.endpoints.length === 0 && server.clocks.length === 0 && server.files.length === 0 && server.actions.length === 0;
      const caps = ev.deployed_capability_names(hash, scheduled) as string[];
      return { items: manifestItems(caps, doc.server?.requirements), optional: doc.server?.requirements?.optional ?? [], trigger: manifestTrigger(caps), byHand: server.endpoints.length === 0 };
    },
    [ev, byName],
  );

  // The server this flow declares, derived from the canvas (canvasOps.deriveServer). Recomputed
  // when the canvas's structure changes, not on moves.
  const derived = useMemo(() => {
    const { nodes: ns, edges: es } = live.current;
    return deriveServer(ns, es, ev, settings.port);
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, settings.port]);

  // What the canvas's flow would ask for, by consent name (null while it doesn't derive): the
  // deploy settings offer each one but its trigger as something it can run without.
  const flowCaps = useMemo(() => {
    try {
      const { nodes: ns, edges: es } = live.current;
      if (ns.length === 0) return null;
      const hash = flowUnitHash(ns, es, ev, settings.port);
      const scheduled = settings.interval.trim() !== "" && derived.endpoints.length === 0 && derived.clocks.length === 0 && derived.files.length === 0 && derived.actions.length === 0;
      return ((ev.deployed_capability_names(hash, scheduled) as string[])).sort();
    } catch {
      return null;
    }
    // live.current is read on purpose, as `derived` above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, settings.port, settings.interval, derived]);
  const flowCapabilities = useMemo(() => flowCaps ?? [], [flowCaps]);
  // Nothing would start a run: no route, clock, file handler or action, nor the old forms'
  // `ingress`, interval or `on file` (docs/L4s/triggers-L4.md fork 14). Deploy is greyed, with
  // the quick fix. Not said of a canvas that doesn't derive: that has its own error.
  const startsNothing =
    flowCaps !== null &&
    derived.endpoints.length + derived.clocks.length + derived.files.length + derived.actions.length === 0 &&
    settings.interval.trim() === "" &&
    !flowCaps.some((c) => c === "NetworkIngress" || c.startsWith("FileArrival("));

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
  // Deploying it, and running, pausing or stopping it (useDeployFlow.ts).
  const { deployedEntry, outOfDate, deployBusy, secretNames, onDeployFlow, onFlowAction } = useDeployFlow({
    flowName,
    setFlowName,
    nodes,
    live,
    derived,
    flowCapabilities,
    settings,
    source,
    getViewport,
    ev,
    dialog,
    deployments,
    refreshDeployments,
    setError: setDeployError,
  });


  // ── Modes: view a deployment, edit the flow (useModeSwitch.ts) ───────────────────────────
  const { onView, onMode } = useModeSwitch({
    flow,
    ev,
    deployments,
    tabs: { activeId, stash, captureTab, showTab, tabOf },
    tabFromDocument,
    canvas: { beforeView, viewDoc, history, restoring, setNodes, setEdges, setViewport, bump },
    setters: { setViewing, setViewTrace, setInspectNode, setSelectedRun, setStep },
    setError: setDeployError,
  });

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
    deployments,
    fitView,
  });

  const onProject = useCallback(() => {
    const result = projectCanvas(nodes, edges, ev, canvasKey);
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
    const result = groupSelection(selected, edges, ev, () => idRef.current++);
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
    const result = expandComposite(target, live.current.edges, ev, () => idRef.current++, knownNames());
    if (result.ok) {
      setNodes((ns) => [...ns.filter((n) => n.id !== target.id), ...result.nodes]);
      setEdges(result.edges);
      setProjError("");
      setErrorNodeIds(new Set());
    } else {
      setProjError(result.message);
    }
  }, [ev, byName, variadic, setNodes, setEdges]);

  // Look inside a composite: its inner flow on a canvas of its own, over this one
  // (useCompositeOverlay.tsx). While it is open, undo and redo are its own.
  const nextId = useCallback(() => idRef.current++, []);
  const compositeOverlay = useCompositeOverlay({
    live,
    ev,
    byName,
    variadic,
    nextId,
    viewing: viewing !== null,
    run: { selectedRun: flow.selectedRun, viewTrace: flow.viewTrace },
    closeOn: activeId,
    setNodes,
    setEdges,
    onMessage: setSaveMsg,
    onError: setProjError,
  });
  const expandNode = compositeOverlay.expand;
  overlayOpen.current = compositeOverlay.open;
  // A sink's quick fix: tie its trigger to the one picked.
  const tie = useCallback(
    (id: string, trigger: { id: string; output: string }) => setEdges((es) => (es.some((e) => e.target === id && e.targetHandle === "trigger") ? es : es.concat(tieEdge(id, trigger)))),
    [setEdges],
  );
  const nodeActions = useMemo(() => ({ updateHolon, expandComposite: expandNode, ungroup, tie }), [updateHolon, expandNode, ungroup, tie]);

  const onExpand = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length !== 1) return; // mirrors the button's own disabled guard, defensively
    const target = selected[0];
    if (target.data.body.kind !== "Composite") return;
    ungroup(target.id);
  }, [nodes, ungroup]);

  // The node the Inspector shows: in view mode the one clicked or stepped to, otherwise the one
  // selected (none when several are). Its unit (via unitFor, as project and group resolve it) is
  // what the unit tab descends into, read only.
  const picked = nodes.filter((n) => n.selected);
  const inspected = viewing ? (nodes.find((n) => n.id === flow.inspectNode) ?? null) : picked.length === 1 ? picked[0] : null;
  // The inspected node's line in the flow as text (docs/LANGUAGE.md): the canvas printed through
  // the language, so a node is read as the expression it is part of.
  const inspectedSource = useMemo(
    () => (inspected ? sourceLineOf(inspected.id, flowName, { nodes: nodes as unknown[], edges: edges as unknown[], server: flow.settings }, ev) : null),
    [inspected, nodes, edges, flow.settings, flowName, ev],
  );
  const inspectedUnit = useMemo(() => {
    if (!inspected) return null;
    try {
      return <UnitTree hash={unitFor(inspected, ev)} ev={ev} />;
    } catch (err) {
      return <div style={{ color: "#e0916f" }}>{err instanceof Error ? err.message : String(err)}</div>;
    }
  }, [inspected, ev]);

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
    // A sink nothing triggers, on a canvas with a trigger: flagged, a fix for each trigger.
    const untied = untiedSinks(nodes, edges);
    const triggers = untied.size > 0 ? triggersOf(nodes) : [];
    if (errorNodeIds.size === 0 && !nodes.some(outdated) && untied.size === 0) return nodes;
    return nodes.map((n) => {
      const flags = { ...(errorNodeIds.has(n.id) ? { error: true } : {}), ...(outdated(n) ? { updateAvailable: true } : {}), ...(untied.has(n.id) ? { untied: triggers } : {}) };
      return Object.keys(flags).length > 0 ? { ...n, data: { ...n.data, ...flags } } : n;
    });
  }, [nodes, edges, errorNodeIds, viewing, view.viewNodes, savedFlowTimes]);

  // Trigger wires are drawn lighter than data wires (canvasOps.lighterTriggers).
  const decoratedEdges = useMemo(() => lighterTriggers(viewing ? view.viewEdges : edges), [viewing, view.viewEdges, edges]);

  // The flow's text beside the canvas (useTextPane.ts, TextPane.tsx), and in view mode the run
  // shown in it.
  const replayed = view.replayed;
  const textRun = useMemo(() => (replayed ? { order: replayed.order, traces: replayed.traces, step: flow.step } : null), [replayed, flow.step]);
  const textPane = useTextPane({ flow, setFlow, nodes, edges, canvasKey, live, setNodes, setEdges, ev, idRef, viewing: viewing !== null, run: textRun, onPick: view.pickNode });

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
    <div className="hl-main">
    {/* Every tab, pages first, then the open flows; each closes. With nothing open (or the last
        flow closed) the canvas shows home, the default page. */}
    <FlowTabs
      tabs={tabList}
      active={shown === "canvas" ? activeId : -1}
      onSelect={switchTab}
      onClose={closeTab}
      pages={start.pages.map((p) => ({
        key: p,
        label: p,
        active: shown === p,
        onSelect: () => start.openPage(p),
        onClose: () => start.closePage(p),
      }))}
    />
    <div className="hl-work">
    {/* The palette is the flow tab's: edit mode places nodes from it; view mode is read only. */}
    {!viewing && shown === "canvas" && (
      // A flow isn't offered inside itself.
      <Palette
        catalog={catalog}
        templates={templateNames()}
        flows={savedFlows.filter((f) => f !== flowName)}
        market={{ url: market.url, error: market.error, entries: market.catalog }}
        docs={docs}
        open={paletteOpen}
        onOpen={onPaletteOpen}
      />
    )}
    <div
      className={["hl-canvas", viewing ? "hl-canvas--viewing" : "", inspectorOpen ? "" : "hl-canvas--inspector-closed", textPane.open ? "hl-canvas--text" : ""].filter(Boolean).join(" ")}
      style={textPane.open ? ({ "--text-width": `${textPane.pane.width}px` } as CSSProperties) : undefined}
      onDrop={onDrop}
      onDragOver={onDragOver}
    >
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
                  onDeploy={onDeployFlow}
                  onAction={onFlowAction}
                  startsNothing={startsNothing}
                  onSettingsOpen={() => onDeployPanelOpen(true)}
                  onMakeRunnable={() => {
                    try {
                      textPane.rewrite(runnableByHand);
                      setDeployError("");
                    } catch (e) {
                      setDeployError(`can't make it runnable by hand: ${e instanceof Error ? e.message : String(e)}`);
                    }
                  }}
                />
              }
              saveMsg={saveMsg}
              error={page === "canvas" ? deployError || listError : ""}
              textOpen={textPane.open}
              onToggleText={textPane.toggle}
            />
          </Panel>
          {!viewing && (
            <Panel position="bottom-right">
              <ProjectionPanel
                nodeCount={nodes.length}
                selectedCount={selectedCount}
                canExpand={canExpand}
                onProject={onProject}
                onGroup={onGroup}
                onExpand={onExpand}
                projection={projection}
                onBoundaryChange={setBoundaryValue}
                onExcludeOutput={excludeOutput}
                error={projError}
              />
            </Panel>
          )}
        </ReactFlow>
        {nodes.length === 0 && page === "canvas" && (
          <div className="hl-canvas__empty">drag a primitive here to place it</div>
        )}
        {compositeOverlay.render(flowName ?? "untitled")}
      </div>
      {textPane.open && <TextPane key={activeId} {...textPane.pane} />}
      <Inspector node={inspected} unit={inspectedUnit} run={inspectorRun(view, flow)} source={inspectedSource} open={inspectorOpen} onOpen={onInspectorOpen} />
      <ViewModePanes view={{ ...view, showNode: (id) => { view.showNode(id); textPane.revealNode(id); } }} flow={flow} nodes={nodes} onRestored={refreshDeployments} />
      {/* The Flows / Deployed pages are overlays ON TOP of the always-mounted canvas — React
          Flow's own state (nodes mid-edit, viewport) lives in it and must survive navigation,
          so the canvas is covered, never unmounted. The deployments list used to live in a
          bottom-left panel on the canvas itself; it moved here wholesale — one home, not two. */}
      {shown !== "canvas" && (
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
          {shown === "flows" && (
            <FlowsPage
              openTabs={tabList}
              onOpen={onOpenFlow}
              onSwitch={switchTab}
              samples={samples()}
              onOpenSample={onOpenSample}
              holonText={{
                onImport: openText,
                exportOf: async (name) => printHolon(name, (await getFlow(name)).document, ev),
                textOfSample: (sample) => sample.text,
              }}
              manifestOf={manifestOfDocument}
              marketplace={{
                url: market.url,
                onChange: market.refresh,
                onPublish: market.publish,
                listings: market.listings,
                adoption: (root, name) => new Set(market.listings.filter((l) => l.name !== name && l.contains.includes(root)).map((l) => l.name)).size,
                onYank: market.yank,
              }}
            />
          )}
          {shown === "admin" && me.role === "admin" && (
            <AdminPage
              me={me.name}
              onOpenDeployment={(slug) => {
                setFocus((f) => ({ slug, n: (f?.n ?? 0) + 1 }));
                start.openPage("deployed");
              }}
            />
          )}
          {shown === "docs" && (
            <Suspense fallback={<div style={{ color: "#7e8ea8" }}>loading the manual…</div>}>
              <ManualPage at={manualAt} onGo={setManualAt} onTry={openText} />
            </Suspense>
          )}
          {shown === "welcome" && (
            <WelcomePage
              name={me.name}
              showOnStart={start.prefs.welcome !== false}
              onShowOnStart={(show) => start.onPrefs({ ...start.prefs, welcome: show })}
              onAction={(a) => (a === "new" ? start.onNew() : start.openPage(a === "samples" ? "flows" : a))}
              onManual={() => start.openPage("docs")}
            />
          )}
          {shown === "home" && (
            <HomePage
              panes={start.prefs.home ?? defaultHome()}
              onPanes={(home) => start.onPrefs({ ...start.prefs, home })}
              deployments={deployments}
              samples={samples()}
              market={{ url: market.url, entries: market.catalog }}
              actions={{
                newFlow: start.onNew,
                openFlow: (name) => getFlow(name).then((rec) => onOpenFlow(rec)).catch((e: unknown) => setDeployError(e instanceof Error ? e.message : String(e))),
                openSample: onOpenSample,
                viewDeployment: onView,
                goTo: start.openPage,
                openWelcome: () => start.openPage("welcome"),
              }}
            />
          )}
          {shown === "deployed" && (
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
              focus={focus}
              role={me.role}
            />
          )}
        </div>
      )}
    </div>
    {shown === "canvas" && (
      <ToolsPanel
        key={flowName ?? ""}
        open={toolsPanelOpen}
        onOpen={onToolsPanelOpen}
        ev={ev}
        endpoints={derived.endpoints}
        clocks={derived.clocks}
        files={derived.files}
        actions={derived.actions}
        errors={derived.errors}
        liveSlug={deployedEntry && deployedEntry.port !== null ? (flowName ?? "") : ""}
        tickSlug={deployedEntry && deployedEntry.status === "running" ? (flowName ?? "") : ""}
        role={me.role}
        outOfDate={outOfDate === true}
        onViewRun={(seq) => flowName && onView(flowName, seq)}
      />
    )}
    {shown === "canvas" && (
      <DeployPanel
        open={deployPanelOpen}
        onOpen={onDeployPanelOpen}
        settings={settings}
        setSettings={setSettings}
        slug={flowName}
        secretNames={secretNames}
        capabilities={flowCapabilities}
        clocks={derived.clocks.map((c) => c.label)}
        fileFolders={derived.files.map((f) => f.folder)}
        readOnly={viewing !== null}
      />
    )}
    </div>
    </div>
    </NodeActions.Provider>
    </NodeDocs.Provider>
  );
}

function Editor({ me }: { me: Me }) {
  // What the work area shows: a page, or the canvas (the active flow's tab).
  const [page, setPage] = useState<AppPage>("home");
  // What the editor keeps for this person (home.ts): read once, saved as it changes. The welcome
  // opens on start unless they said not to; otherwise the editor starts at home.
  const [prefs, setPrefsState] = useState<Prefs>({});
  // The pages open in tabs of their own, in the order opened; home to begin with, and the
  // welcome unless they said not to.
  const [pages, setPages] = useState<PageTab[]>(["home"]);
  useEffect(() => {
    getPrefs()
      .then((raw) => {
        const p = readPrefs(raw);
        setPrefsState(p);
        if (p.welcome !== false) {
          setPages((ps) => (ps.includes("welcome") ? ps : [...ps, "welcome"]));
          // Shown only if they are still where the editor started: prefs that arrive after
          // they opened something else don't take them away from it.
          setPage((shown) => (shown === "home" ? "welcome" : shown));
        }
      })
      .catch(() => undefined);
  }, []);
  /** Show page `p`, in its tab (opened if it isn't). */
  const openPage = useCallback((p: PageTab) => {
    setPages((ps) => (ps.includes(p) ? ps : [...ps, p]));
    setPage(p);
  }, []);
  /** Close page `p`'s tab; if it was showing, the tab beside it shows, else the open flow (or,
   *  with none, home as the default page). */
  const closePage = useCallback(
    (p: PageTab) => {
      const i = pages.indexOf(p);
      const left = pages.filter((x) => x !== p);
      setPages(left);
      if (page === p) setPage(left[i] ?? left[i - 1] ?? "canvas");
    },
    [pages, page],
  );
  const onPrefs = useCallback((p: Prefs) => {
    setPrefsState(p);
    setPrefs(p).catch(() => undefined);
  }, []);
  const [catalog, setCatalog] = useState<WUnit[] | null>(null);
  // What each primitive does, from core: on each node, the palette and the component tab.
  const [docs, setDocs] = useState<Map<string, string>>(new Map());
  const [variadic, setVariadic] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string>("");
  // "New" asks the canvas for an untitled draft in a tab of its own; `nonce` makes each New take effect.
  const [pendingNew, setPendingNew] = useState<{ nonce: number } | null>(null);

  useEffect(() => {
    (async () => {
      await ensureReady();
      setDocs(primitiveDocs());
      setCatalog(primitiveCatalog());
      setVariadic(variadicPrimitives());
    })().catch((e: unknown) => setError(String(e)));
  }, []);

  const onNew = () => {
    setPendingNew((p) => ({ nonce: (p?.nonce ?? 0) + 1 }));
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
          <button style={menuBtn(page === "flows")} onClick={() => openPage("flows")}>
            flows
          </button>
          <button style={menuBtn(page === "deployed")} onClick={() => openPage("deployed")}>
            deployed
          </button>
          <button style={menuBtn(page === "docs")} onClick={() => openPage("docs")}>
            docs
          </button>
          {me.role === "admin" && (
            <button style={menuBtn(page === "admin")} onClick={() => openPage("admin")}>
              admin
            </button>
          )}
        </nav>
        <AccountMenu me={me} style={menuBtn(false)} onWelcome={() => openPage("welcome")} />
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
                me={me}
                start={{ prefs, onPrefs, pages, openPage, closePage, onNew }}
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
      <AuthGate>{(me) => <Editor me={me} />}</AuthGate>
    </DialogProvider>
  );
}