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
  describeItem,
} from "./holon";
import { toDecide } from "./decisions";
import { changeDeployment, deleteDeployment, getDeployedDocument, getFlow, getManifest, getPrefs, setPrefs, type FlowDocument, type FlowRecord } from "./server";
import { mayDiff } from "./mayItems";
import type { MayHere } from "./mayHere";
import { MayStrip } from "./MayStrip";
import { outputColor } from "./theme";
import { Palette, DND_MIME, FLOW_PREFIX } from "./Palette";
import { PrimitiveNode, type PrimitiveNodeType } from "./PrimitiveNode";
import { UnitTree } from "./UnitTree";
import { Inspector } from "./ViewPanes";
import { structuralKey } from "./flowDoc";
import { knownNames, rememberName, restoreDocument } from "./docUnits";
import { DialogProvider, useDialog } from "./Dialog";
import { ToastProvider, useToast } from "./ui/Toast";
import { ToolsPanel } from "./ServerPanel";
import { DeployBar, DeployPanel, DEFAULT_SETTINGS } from "./DeployBar";
import { templateNames } from "./templates";
import { primitiveName, stageClosure, unitFor } from "./canvasUnits";
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
import { tidyPositions } from "./canvasTidy";
import { FlowPage } from "./FlowPage";
import { pageOf, routeOfPage, shownAt, type AppPage } from "./Pages";
import { EditorHome } from "./EditorHome";
import { FlowsPage } from "./FlowsPage";
import { ApprovalsPage } from "./ApprovalsPage";
import { ReviewPage } from "./ReviewPage";
import { useApprovals } from "./useApprovals";
import { LibraryPage } from "./LibraryPage";
import { format, parentOf, same, type Route } from "./routes";
import { PAGE_KEYS } from "./keymap";
import { KeysDialog, useKeys } from "./ui/Keys";
import { CommandPalette } from "./ui/CommandPalette";
import type { ColonCommand, Command } from "./commands";
import { useHashRoute, type Navigate } from "./useHashRoute";
import { applyTheme, applyUi, storedTheme, storedUi, useDark, type Ui } from "./look";
import { useCanvasHistory } from "./useCanvasHistory";
import { useDeployments } from "./useDeployments";
import { buildSample, samples, type Sample } from "./samples";
import { blankTab, freshTab, metaSetter, savedKeyOf, sourceToSave, syncKeyOf, tabFromDocument as tabFromDoc, tabKind, type FlowMeta, type Mode, type TabState } from "./tabs";
import { NodeActions, NodeDocs, nodeDescriber } from "./NodeDocs";
import { AuthGate, type Me } from "./AuthGate";
import { Shell } from "./ui/Shell";
import { OwnedPage, PeoplePage, PolicyPage, ResourcesPage } from "./HostPages";
import { OverviewPage } from "./OverviewPage";
import { compileHolon, documentText, printHolon, sourceLineOf } from "./holonText";
import { askText, TextPane, type TextAsk } from "./TextPane";
import type { Inspected } from "./holonText";
import { useTextPane } from "./useTextPane";
import { WelcomePage } from "./WelcomePage";
import type { ManualAt } from "./ManualPage";
import { readPrefs, withPane, type EditorPane, type Prefs } from "./home";
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
  route,
  navigate,
  pendingNew,
  docs,
  me,
  start,
}: {
  catalog: WUnit[];
  variadic: Set<string>;
  /** Who is signed in (AuthGate.tsx). */
  me: Me;
  /** What the editor keeps with the account (home.ts): home's panes, the welcome, the look,
   *  which of the editor's panes are hidden. */
  start: {
    prefs: Prefs;
    onPrefs: (p: Prefs) => void;
    onNew: () => void;
    /** Show every key (the ? dialog). */
    showKeys: () => void;
    /** Whether the command palette is open (the top bar opens it too), and opening or closing it. */
    finding: boolean;
    onFinding: (open: boolean) => void;
    /** How many things wait for a person, told when it changes (the nav's count). */
    onWaiting: (n: number) => void;
  };
  page: AppPage;
  setPage: (p: AppPage) => void;
  /** Where the address says the editor is (routes.ts), and going somewhere. */
  route: Route;
  navigate: Navigate;
  pendingNew: { nonce: number } | null;
  docs: Map<string, string>;
}) {
  const dialog = useDialog();
  const toast = useToast();
  const dark = useDark();
  const [nodes, setNodes, onNodesChange] = useNodesState<PrimitiveNodeType>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { screenToFlowPosition, getNode, getNodes, getEdges, getViewport, setViewport, fitView, getInternalNode } =
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
  // Which of the editor's panes show (`c i v b`; the text is each tab's own), kept with the
  // account (home.ts).
  const { prefs, onPrefs } = start;
  const paneShown = (p: EditorPane) => !(prefs.hidden ?? []).includes(p);
  const showPane = useCallback((p: EditorPane, shown: boolean) => onPrefs({ ...prefs, hidden: withPane(prefs.hidden, p, shown) }), [prefs, onPrefs]);
  const inspectorOpen = paneShown("inspector");
  const onInspectorOpen = useCallback((open: boolean) => showPane("inspector", open), [showPane]);
  // On a phone the side panels lie over the canvas (styles.css), so they start folded there.
  const [paletteOpen, setPaletteOpen] = useState(() => {
    if (phone()) return false;
    try { return localStorage.getItem("holon.palette") !== "closed"; } catch { return true; }
  });
  const onPaletteOpen = useCallback((open: boolean) => {
    setPaletteOpen(open);
    try { localStorage.setItem("holon.palette", open ? "open" : "closed"); } catch { /* ignore */ }
  }, []);
  const [deployPanelOpen, setDeployPanelOpen] = useState(() => {
    try { return localStorage.getItem("holon.deploy") === "open"; } catch { return false; }
  });
  const onDeployPanelOpen = useCallback((open: boolean) => {
    setDeployPanelOpen(open);
    try { localStorage.setItem("holon.deploy", open ? "open" : "closed"); } catch { /* ignore */ }
  }, []);
  const [toolsPanelOpen, setToolsPanelOpen] = useState(() => {
    if (phone()) return false;
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

  // The last deploy or deployment error, shown in the toolbar and on the flows page.
  const [deployError, setDeployError] = useState("");
  // The deployment card asked for from the admin page's permissions, at its `n`th asking.
  const [focus, setFocus] = useState<{ slug: string; n: number } | null>(null);
  const focusSlug = route.kind === "flow" ? route.slug : null;
  useEffect(() => {
    if (focusSlug !== null) setFocus((f) => ({ slug: focusSlug, n: (f?.n ?? 0) + 1 }));
  }, [focusSlug]);
  // Where the docs tab is in the manual (null: its index): kept here, so it is where it was when
  // the tab shows again.
  // The manual's page is the address's (#docs.<page>@<heading>): a link in it is a navigation.
  const manualAt: ManualAt | null = route.kind === "docs" && route.page !== null ? { page: route.page, anchor: route.anchor } : null;
  const setManualAt = useCallback((at: ManualAt) => navigate({ kind: "docs", page: at.page, anchor: at.anchor }), [navigate]);

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
  const { name: flowName, settings, savedKey, notice, viewing } = flow;
  const setFlowName = useMemo(() => metaSetter(setFlow, "name"), []);
  const setSavedOnServer = useMemo(() => metaSetter(setFlow, "saved"), []);
  const setSettings = useMemo(() => metaSetter(setFlow, "settings"), []);
  const setSavedKey = useMemo(() => metaSetter(setFlow, "savedKey"), []);
  const setNotice = useMemo(() => metaSetter(setFlow, "notice"), []);
  const setViewing = useMemo(() => metaSetter(setFlow, "viewing"), []);
  const setViewUnit = useMemo(() => metaSetter(setFlow, "viewUnit"), []);
  const setViewTrace = useMemo(() => metaSetter(setFlow, "viewTrace"), []);
  const setSelectedRun = useMemo(() => metaSetter(setFlow, "selectedRun"), []);
  const setStep = useMemo(() => metaSetter(setFlow, "step"), []);
  const setInspectNode = useMemo(() => metaSetter(setFlow, "inspectNode"), []);
  // What opening a flow found wrong is said once, when its tab is shown, and then forgotten.
  useEffect(() => {
    if (!notice) return;
    toast(notice, "bad");
    setNotice("");
  }, [notice, toast, setNotice]);
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
  // Tidy: the shared layout (graphLayout.ts) lays the nodes out, as the VS Code graph does. It moves the
  // nodes only, not the view; the move is one undoable step. Fit is its own button.
  const tidyCanvas = useCallback(() => {
    // The layout reads each node's drawn size: the page's own box for the node, which is what it occupies.
    const sized = live.current.nodes.map((n) => {
      const el = document.querySelector<HTMLElement>(`.react-flow__node[data-id="${CSS.escape(n.id)}"]`);
      const measured = el ? { width: el.offsetWidth, height: el.offsetHeight } : (getInternalNode(n.id)?.measured ?? n.measured);
      return measured ? { ...n, measured } : n;
    });
    const at = tidyPositions(sized, live.current.edges);
    setNodes((ns) => ns.map((n) => ({ ...n, position: at.get(n.id) ?? n.position })));
  }, [setNodes, getInternalNode]);
  const fitCanvas = useCallback(() => {
    fitView({ padding: 0.1, duration: 200 });
  }, [fitView]);
  // The text a save keeps with the flow (the pane's, as typed, while it stands for the canvas).
  const source = useMemo(() => sourceToSave(flow.text, syncKeyOf(nodes, edges, settings, flowName)), [flow.text, nodes, edges, settings, flowName]);
  const dirty = useMemo(
    // While viewing, what counts is whether the flow set aside had unsaved changes.
    () => (viewing ? (beforeView.current?.dirty ?? false) : savedKeyOf(nodes, edges, settings, source) !== savedKey),
    [nodes, edges, settings, source, savedKey, viewing],
  );

  // ── Tabs: one per open flow (useTabs.ts) ──────────────────────────────────────────────────
  const { tabIds, activeId, stash, captureTab, showTab, switchTab, tabOf, closeTab, closed, reopen } = useTabs({
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
  // What the work area shows: the page asked for; in the editor, its Home tab when asked for or
  // with no flow open.
  const shown: AppPage = page === "canvas" && (route.kind === "editor-home" || tabIds.length === 0) ? "editor-home" : page;

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
    notify: toast,
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
      const caps = ev.deployed_capability_names(hash) as string[];
      // Its unrecorded handlers, each an item to approve (the server reads them from the unit).
      const unrecorded = [...server.endpoints.map((e) => ({ label: `${e.method} ${e.path}`, log: e.log })), ...server.clocks, ...server.files, ...server.mails, ...server.feeds, ...server.queues, ...server.actions]
        .filter((h) => h.log === "unrecorded")
        .map((h) => h.label);
      return { items: manifestItems(caps, { ...doc.server?.requirements, unrecorded }), optional: doc.server?.requirements?.optional ?? [], trigger: manifestTrigger(caps) };
    },
    [ev, byName],
  );

  // A saved flow made ready to deploy from its review (ReviewPage.tsx), as deploying it from the
  // canvas would: its units put back, its server derived from them and staged on the server.
  const prepareSaved = useCallback(
    async (_slug: string, doc: FlowDocument) => {
      restoreDocument(doc, ev);
      const server = deriveServer(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, doc.server?.port ?? DEFAULT_SETTINGS.port);
      if (server.errors.length > 0) throw new Error(server.errors.join("; "));
      if (!server.serverHash) throw new Error("nothing would start a run: it has no route, clock, file handler or action");
      await stageClosure(ev, server.serverHash, new Set());
      return { hash: server.serverHash, document: { ...doc, trace: server.trace } };
    },
    [ev],
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
      return (ev.deployed_capability_names(hash) as string[]).sort();
    } catch {
      return null;
    }
    // live.current is read on purpose, as `derived` above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, settings.port]);
  const flowCapabilities = useMemo(() => flowCaps ?? [], [flowCaps]);
  // The queues it sends to, by name: the server panel says what their messages came to.
  const sendsTo = useMemo(() => flowCapabilities.flatMap((c) => /^QueueSend\("(.*)"\)$/.exec(c)?.[1] ?? []), [flowCapabilities]);
  // What each node does with a secret (a request it is sent in, a write the host refuses, the
  // route's answer), by node id: the canvas marks them.
  const secretMarks = useMemo(() => {
    try {
      const { nodes: ns, edges: es } = live.current;
      return ns.length === 0 ? {} : ((ev.secret_marks(ns, es, settings.port) ?? {}) as Record<string, string[]>);
    } catch {
      return {} as Record<string, string[]>;
    }
    // live.current is read on purpose, as `derived` above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, settings.port]);
  // Nothing would start a run: no route, clock, file, mail, feed or queue handler or action (docs/L4s/triggers-L4.md
  // fork 14). Deploy is greyed, saying what to add. Not said of a canvas that doesn't derive: that
  // has its own error.
  const startsNothing = flowCaps !== null && derived.endpoints.length + derived.clocks.length + derived.files.length + derived.mails.length + derived.feeds.length + derived.queues.length + derived.websockets.length + derived.actions.length === 0;

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
  const { deployments, listError, refresh: refreshDeployments } = useDeployments(page);
  // What waits for a person (approvals.ts): Approvals lists it, the nav counts it.
  const approvals = useApprovals(deployments, { shown: page === "approvals" || page === "home", asks: page === "approvals" });
  const { onWaiting } = start;
  // With the approvals among the runs waiting that are this person's to decide (decisions.ts).
  const deciding = useMemo(() => toDecide(approvals.runs ?? [], me), [approvals.runs, me]);
  useEffect(() => onWaiting(approvals.approvals.length + deciding), [approvals.approvals.length, deciding, onWaiting]);
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
    onDeployed: () => onDeployPanelOpen(true),
  });

  // The It may strip (MayStrip.tsx): the draft's manifest, as the server will read it, beside the
  // deployed version's, read again when another version is deployed. Worked out when the canvas's
  // structure changes, as the unit it compiles to does.
  const draftItems = useMemo(() => {
    if (flowCaps === null) return null;
    const handlers = [...derived.endpoints.map((e) => ({ label: `${e.method} ${e.path}`, log: e.log })), ...derived.clocks, ...derived.files, ...derived.actions];
    const unrecorded = handlers.filter((h) => h.log === "unrecorded").map((h) => h.label);
    return manifestItems(flowCaps, { ...settings.requirements, unrecorded });
  }, [flowCaps, derived, settings.requirements]);
  const deployedUnit = deployedEntry?.unit;
  const [deployedItems, setDeployedItems] = useState<{ unit: string; items: string[] } | null>(null);
  useEffect(() => {
    if (!flowName || !deployedUnit) return;
    let fresh = true;
    getManifest(flowName).then(
      (m) => fresh && setDeployedItems({ unit: deployedUnit, items: m.items }),
      () => fresh && setDeployedItems(null),
    );
    return () => {
      fresh = false;
    };
  }, [flowName, deployedUnit]);
  const mayItems = useMemo(() => {
    if (draftItems === null) return null;
    return mayDiff(draftItems, deployedUnit && deployedItems?.unit === deployedUnit ? deployedItems.items : null);
  }, [draftItems, deployedUnit, deployedItems]);


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

  // ── The address and the editor's flow (routes.ts) ─────────────────────────────────────────
  // `#editor.<slug>[.view[.run-<seq>]]` says which flow the editor shows, and how. Asked for (a
  // link, Back, a reload), the editor goes there: to its tab, else its saved copy, else its
  // deployment. Changed here (a tab picked, view mode, a run), the address follows, replaced
  // rather than added to, so Back leaves the editor rather than stepping through its clicks.
  // While a place asked for is being reached, the address waits for it.
  const here = useMemo<Extract<Route, { kind: "editor" }>>(
    () => ({ kind: "editor", slug: flow.viewing ?? flow.name, view: flow.viewing !== null, seq: flow.viewing !== null ? (flow.selectedRun?.seq ?? null) : null }),
    [flow.viewing, flow.name, flow.selectedRun],
  );
  const hereNow = useRef(here);
  hereNow.current = here;
  const reaching = useRef(false);
  const [reached, setReached] = useState(0);
  const routeKey = format(route);
  useEffect(() => {
    if (route.kind !== "editor" || route.slug === null || same(route, hereNow.current)) return;
    const want = route;
    const slug = want.slug!;
    const at = hereNow.current;
    reaching.current = true;
    (async () => {
      if (want.view) return onView(slug, want.seq ?? undefined);
      if (at.slug === slug && at.view) return onMode("edit");
      const open = tabOf(slug);
      if (open !== null) return switchTab(open);
      const rec = await getFlow(slug).catch(() => null);
      if (rec) return void (await onOpenFlow(rec));
      if (deployments.some((d) => d.slug === slug)) return onView(slug);
      toast(`There is no flow named "${slug}" to open.`, "bad");
    })()
      .catch((e: unknown) => toast(e instanceof Error ? e.message : String(e), "bad"))
      .finally(() => {
        reaching.current = false;
        setReached((n) => n + 1);
      });
    // Only a new address is gone to: the callbacks change with every tab.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routeKey]);
  const hereKey = format(here);
  useEffect(() => {
    if (shown !== "canvas" || reaching.current) return;
    if (route.kind === "editor" && same(route, here)) return;
    navigate(here, { replace: true });
    // The address is written from where the editor is, not from what it is told.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hereKey, routeKey, shown, reached]);

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
    showValues: paneShown("values"),
    fitView,
  });

  // The panes (gaps-web.md G17): the canvas and the text, one of them always shown; the
  // Inspector; in view mode the values beside each output and the runs below.
  const canvasShown = paneShown("canvas");
  const toggleCanvas = () => {
    if (canvasShown && !textPane.open) textPane.toggle();
    showPane("canvas", !canvasShown);
  };
  const toggleText = () => {
    if (textPane.open && !canvasShown) showPane("canvas", true);
    textPane.toggle();
  };
  const canView = deployedEntry !== undefined;
  // On a phone (styles.css, under 560 px) the editor shows one pane at a time: the switcher in its
  // bar picks which, opening it when it is closed.
  const [onePane, setOnePane] = useState<"canvas" | "text" | "inspector">("canvas");
  const showOnePane = (p: "canvas" | "text" | "inspector") => {
    setOnePane(p);
    if (p === "canvas" && !canvasShown) showPane("canvas", true);
    if (p === "text" && !textPane.open) textPane.toggle();
    if (p === "inspector" && !inspectorOpen) onInspectorOpen(true);
  };

  // The flow's keys (keymap.ts): saved and deployed while it is edited, from the text too; its
  // mode and panes; its values and runs shown or hidden while viewed. Stepping is the bottom
  // pane's (ViewPanes.tsx).
  useKeys(
    {
      "ctrl+s": () => void onSave(),
      "ctrl+d": () => void onDeployFlow(),
    },
    { enabled: shown === "canvas" && !viewing, whileTyping: ["ctrl+s", "ctrl+d"] },
  );
  useKeys(
    {
      e: () => (viewing ? onMode("edit") : canView ? onMode("view") : false),
      c: toggleCanvas,
      x: toggleText,
      i: () => onInspectorOpen(!inspectorOpen),
      v: () => (viewing ? showPane("values", !paneShown("values")) : false),
      b: () => (viewing ? showPane("bottom", !paneShown("bottom")) : false),
    },
    { enabled: shown === "canvas" },
  );

  // ── The command palette (ui/CommandPalette.tsx): Ctrl+K or / ─────────────────────────────
  // Its entries are what there is now: the pages, every deployed or saved flow and its tabs, the
  // flow edited's actions, the samples, the looks.
  const { finding, onFinding: setFinding } = start;
  // The name at the text's cursor, for the Inspector (TextPane's onInspect).
  const [word, setWord] = useState<Inspected | null>(null);
  const editing = shown === "canvas" && !viewing;
  const commands = useMemo<Command[]>(() => {
    if (!finding) return [];
    const go = (to: Route) => () => navigate(to);
    const name = flowName || "this flow";
    const out: Command[] = [];
    if (editing) {
      out.push({ label: `Save ${name}`, kind: "this flow", run: () => void onSave() });
      out.push({ label: `Deploy ${name}`, kind: "this flow", run: () => void onDeployFlow() });
      out.push({ label: `Format ${name}`, kind: "this flow", run: () => askTextOpening({ do: "format" }) });
    }
    const pages: [string, Route][] = [
      ["Overview", { kind: "overview" }],
      ["Approvals", { kind: "approvals", tab: "deploys" }],
      ["Flows", { kind: "flows" }],
      ["Editor", { kind: "editor", slug: null, view: false, seq: null }],
      ["Library", { kind: "library", tab: "samples" }],
      ...(me.role === "admin"
        ? ([
            ["Resources", { kind: "resources", tab: "databases" }],
            ["People and access", { kind: "people", tab: null }],
            ["Policy", { kind: "policy" }],
          ] as [string, Route][])
        : me.kind === "person"
          ? ([["Your agents and services", { kind: "people", tab: null }]] as [string, Route][])
          : []),
      ["Docs", { kind: "docs", page: null, anchor: null }],
      ["Welcome", { kind: "start" }],
    ];
    for (const [label, to] of pages) out.push({ label, kind: "page", run: go(to) });
    const deployed = new Set(deployments.map((d) => d.slug));
    for (const { slug } of deployments) {
      out.push({ label: slug, kind: "flow", run: go({ kind: "flow", slug, tab: "status" }) });
      for (const tab of ["runs", "history", "state", "settings"] as const) out.push({ label: `${slug} ${tab}`, kind: "flow", run: go({ kind: "flow", slug, tab }) });
      out.push({ label: `Edit ${slug}`, kind: "flow", run: go({ kind: "editor", slug, view: false, seq: null }) });
      out.push({ label: `View ${slug}`, kind: "flow", run: go({ kind: "editor", slug, view: true, seq: null }) });
    }
    for (const f of savedFlows) if (!deployed.has(f)) out.push({ label: `Edit ${f}`, kind: "saved", run: go({ kind: "editor", slug: f, view: false, seq: null }) });
    out.push({ label: "New draft", kind: "editor", run: start.onNew });
    for (const q of approvals.queue) if (q.version !== null) out.push({ label: `Approve ${q.slug} v${q.version}`, kind: "review", run: go({ kind: "review", slug: q.slug, version: q.version }) });
    for (const s of samples()) out.push({ label: `New from ${s.title}`, kind: "sample", run: () => void onOpenSample(s) });
    for (const theme of ["auto", "light", "dark"] as const) out.push({ label: `Theme: ${theme}`, kind: "look", run: () => start.onPrefs({ ...start.prefs, theme }) });
    out.push({ label: "Interface: Clean", kind: "look", run: () => start.onPrefs({ ...start.prefs, ui: "clean" }) });
    out.push({ label: "Interface: Terminal", kind: "look", run: () => start.onPrefs({ ...start.prefs, ui: "terminal" }) });
    out.push({ label: "Keys", kind: "help", run: start.showKeys });
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [finding, editing, flowName, deployments, savedFlows, me.role, me.kind, start.prefs, approvals.queue]);
  // The text is opened for a `:12` or `:fmt` asked while it is closed, and asked once it is there.
  const askTextOpening = (ask: TextAsk) => {
    if (askText(ask)) return;
    if (!textPane.open) textPane.toggle();
    let tries = 0;
    const again = () => {
      if (!askText(ask) && ++tries < 20) setTimeout(again, 25);
    };
    setTimeout(again, 0);
  };
  const onColon = (c: ColonCommand): string | null => {
    switch (c.kind) {
      case "line":
      case "fmt":
        if (shown !== "canvas") return "open a flow in the editor first";
        askTextOpening(c.kind === "line" ? { do: "line", line: c.line } : { do: "format" });
        return null;
      case "save":
      case "deploy":
        if (!editing) return "edit a flow first";
        void (c.kind === "save" ? onSave() : onDeployFlow());
        return null;
      case "new":
        start.onNew();
        return null;
      case "restore":
        if (!viewing) return "view a deployed flow first";
        navigate({ kind: "flow", slug: viewing, tab: "history" });
        return null;
      case "keys":
        start.showKeys();
        return null;
      case "theme":
        start.onPrefs({ ...start.prefs, theme: c.theme });
        return null;
    }
  };

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
    onMessage: toast,
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
  // What the inspected node lets the flow do, in edit mode: its unit's capabilities as the
  // manifest says them, each marked when it is new against the deployed version (mayHere.ts).
  const inspectedMay = useMemo((): MayHere[] | null => {
    if (!inspected || viewing) return null;
    try {
      const caps = (ev.capability_names(unitFor(inspected, ev)) as string[]).sort();
      const fresh = new Set((mayItems ?? []).filter((m) => m.change === "new").map((m) => m.item));
      return manifestItems(caps).map((item) => ({ item, said: describeItem(item), isNew: fresh.has(item) }));
    } catch {
      return null;
    }
  }, [inspected, viewing, ev, mayItems]);
  const inspectedUnit = useMemo(() => {
    if (!inspected) return null;
    try {
      return <UnitTree hash={unitFor(inspected, ev)} ev={ev} />;
    } catch (err) {
      return <div style={{ color: "var(--bad)" }}>{err instanceof Error ? err.message : String(err)}</div>;
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
    const marked = Object.keys(secretMarks).length > 0;
    if (errorNodeIds.size === 0 && !nodes.some(outdated) && untied.size === 0 && !marked) return nodes;
    return nodes.map((n) => {
      const flags = {
        ...(errorNodeIds.has(n.id) ? { error: true } : {}),
        ...(outdated(n) ? { updateAvailable: true } : {}),
        ...(untied.has(n.id) ? { untied: triggers } : {}),
        ...(secretMarks[n.id] ? { secretSinks: secretMarks[n.id] } : {}),
      };
      return Object.keys(flags).length > 0 ? { ...n, data: { ...n.data, ...flags } } : n;
    });
  }, [nodes, edges, errorNodeIds, viewing, view.viewNodes, savedFlowTimes, secretMarks]);

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

  // The flows the editor's Home offers to open: every one saved or deployed, by name.
  const homeFlows = [...new Set([...savedFlows, ...deployments.map((d) => d.slug)])].sort().map((name) => ({
    name,
    deployed: deployments.some((d) => d.slug === name),
    saved: savedFlows.includes(name),
    open: tabList.some((t) => t.name === name),
  }));

  return (
    <NodeDocs.Provider value={describeNode}>
    <NodeActions.Provider value={nodeActions}>
    <div className="hl-main">
    {finding && <CommandPalette commands={commands} onColon={onColon} onClose={() => setFinding(false)} />}
    {/* The editor's tabs: its Home, then the open flows, each closing; + starts a draft. With
        no flow open (or the last one closed) the editor shows its Home. */}
    {(shown === "canvas" || shown === "editor-home") && (
      <FlowTabs
        tabs={tabList}
        active={shown === "canvas" ? activeId : -1}
        home={shown === "editor-home"}
        onHome={() => navigate({ kind: "editor-home" })}
        onSelect={switchTab}
        onClose={closeTab}
        onNew={start.onNew}
      />
    )}
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
    <div className="hl-edit">
    {!viewing && shown === "canvas" && <MayStrip items={mayItems} deployed={deployedUnit !== undefined} />}
    <div
      className={[
        "hl-canvas",
        viewing ? "hl-canvas--viewing" : "",
        viewing && !paneShown("bottom") ? "hl-canvas--no-bottom" : "",
        inspectorOpen ? "" : "hl-canvas--inspector-closed",
        textPane.open ? "hl-canvas--text" : "",
        canvasShown || !textPane.open ? "" : "hl-canvas--no-canvas",
      ]
        .filter(Boolean)
        .join(" ")}
      style={textPane.open ? ({ "--text-width": `${textPane.pane.width}px` } as CSSProperties) : undefined}
      data-pane={onePane}
      onDrop={onDrop}
      onDragOver={onDragOver}
    >
      {/* The editor's bar (gaps-web.md G17): the flow, its mode, the panes, save and deploy. */}
      <div className="hl-canvas__bar">
        <FlowToolbar
          histVersion={histVersion}
          mode={viewing ? "view" : "edit"}
          canView={canView}
          onMode={onMode}
          panes={[
            { key: "c", name: "canvas", on: canvasShown || !textPane.open, toggle: toggleCanvas },
            { key: "x", name: "text", on: textPane.open, toggle: toggleText },
            { key: "i", name: "inspector", on: inspectorOpen, toggle: () => onInspectorOpen(!inspectorOpen) },
            ...(viewing
              ? [
                  { key: "v", name: "values", on: paneShown("values"), toggle: () => showPane("values", !paneShown("values")) },
                  { key: "b", name: "runs", on: paneShown("bottom"), toggle: () => showPane("bottom", !paneShown("bottom")) },
                ]
              : []),
          ]}
          flowName={flowName}
          dirty={dirty}
          onSave={onSave}
          canUndo={history.current.past.length > 0}
          canRedo={history.current.future.length > 0}
          onUndo={undo}
          onRedo={redo}
          onTidy={tidyCanvas}
          onFit={fitCanvas}
          deployBar={
            <DeployBar
              entry={deployedEntry}
              outOfDate={outOfDate}
              busy={deployBusy}
              onDeploy={onDeployFlow}
              onAction={onFlowAction}
              startsNothing={startsNothing}
              onSettingsOpen={() => onDeployPanelOpen(true)}
            />
          }
          error={page === "canvas" ? deployError || listError : ""}
        />
        <span className="hl-onepane" role="group" aria-label="pane shown">
          {(["canvas", "text", "inspector"] as const).map((p) => (
            <button key={p} aria-pressed={onePane === p} onClick={() => showOnePane(p)}>
              {p}
            </button>
          ))}
        </span>
      </div>
      <div className="hl-canvas__flow">
        <ReactFlow<PrimitiveNodeType>
          colorMode={dark ? "dark" : "light"}
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
          connectionLineStyle={{ stroke: "var(--accent)", strokeWidth: 1.6 }}
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
          <Background variant={BackgroundVariant.Lines} gap={26} color="var(--border)" />
          <Controls showInteractive={false} />
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
      {textPane.open && <TextPane key={activeId} {...textPane.pane} onInspect={setWord} />}
      <Inspector
        node={inspected && secretMarks[inspected.id] ? { ...inspected, data: { ...inspected.data, secretSinks: secretMarks[inspected.id] } } : inspected}
        unit={inspectedUnit}
        run={inspectorRun(view, flow)}
        source={inspectedSource}
        may={inspectedMay}
        open={inspectorOpen}
        onOpen={onInspectorOpen}
        word={word}
        onManual={() => navigate({ kind: "docs", page: "language/reference.md", anchor: null })}
        onSample={(name) => {
          const s = samples().find((x) => x.name === name);
          if (s) void onOpenSample(s);
        }}
      />
      {paneShown("bottom") && <ViewModePanes view={{ ...view, showNode: (id) => { view.showNode(id); textPane.revealNode(id); } }} flow={flow} nodes={nodes} onRestored={refreshDeployments} />}
      {/* The pages are overlays ON TOP of the always-mounted canvas — React
          Flow's own state (nodes mid-edit, viewport) lives in it and must survive navigation,
          so the canvas is covered, never unmounted. The deployments list used to live in a
          bottom-left panel on the canvas itself; it moved here wholesale — one home, not two. */}
      {shown !== "canvas" && (
        <div className="hl-pagebox">
          {shown === "editor-home" && (
            <EditorHome
              flows={homeFlows}
              closed={closed.map((t) => ({
                name: t.meta.name,
                kind: tabKind(t.meta, (name) => deployments.some((d) => d.slug === name)),
                viewing: t.meta.viewing !== null,
                dirty: t.dirty,
              }))}
              navigate={navigate}
              onNew={start.onNew}
              onReopen={reopen}
              onKeys={start.showKeys}
            />
          )}
          {shown === "flows" && (
            <FlowsPage
              deployments={deployments}
              error={deployError || listError}
              openTabs={tabList}
              onOpen={onOpenFlow}
              onSwitch={switchTab}
              onNew={start.onNew}
              navigate={navigate}
              onImport={openText}
              exportOf={async (name) => printHolon(name, (await getFlow(name)).document, ev)}
              manifestOf={manifestOfDocument}
              marketplace={{ url: market.url, onPublish: market.publish }}
            />
          )}
          {shown === "approvals" && route.kind === "approvals" && (
            <ApprovalsPage
              tab={route.tab}
              approvals={approvals.approvals}
              deployments={deployments}
              runs={approvals.runs}
              me={me}
              navigate={navigate}
              onDeclined={approvals.refresh}
              onDecided={() => {
                approvals.refresh();
                refreshDeployments();
              }}
            />
          )}
          {shown === "approvals" && route.kind === "review" && (
            <ReviewPage
              key={route.slug}
              slug={route.slug}
              version={route.version}
              queue={approvals.queue}
              deployments={deployments}
              navigate={navigate}
              prepare={prepareSaved}
              textOf={(slug, doc) => documentText(slug, doc)}
              onKeep={approvals.keep}
              onDeclined={approvals.refresh}
              onDeployed={(slug, version) => {
                refreshDeployments();
                approvals.refresh();
                toast(`Deployed ${slug} v${version}.`, "ok");
                navigate({ kind: "flow", slug, tab: "status" });
              }}
            />
          )}
          {shown === "library" && route.kind === "library" && (
            <LibraryPage
              tab={route.tab}
              navigate={navigate}
              samples={samples()}
              itemsOf={(sample) => manifestOfDocument(buildSample(sample, ev)).items}
              onOpenSample={onOpenSample}
              textOfSample={(sample) => sample.text}
              marketplace={{
                url: market.url,
                onChange: market.refresh,
                listings: market.listings,
                adoption: (root, name) => new Set(market.listings.filter((l) => l.name !== name && l.contains.includes(root)).map((l) => l.name)).size,
                onYank: market.yank,
              }}
            />
          )}
          {/* The host's pages, an admin's (HostPages.tsx). */}
          {(shown === "resources" || (shown === "people" && me.kind !== "person") || shown === "policy") && me.role !== "admin" && <p className="hl-flow__note">The host&apos;s pages are an admin&apos;s.</p>}
          {/* A person who isn't an admin: the agents and services they own. */}
          {shown === "people" && me.role !== "admin" && me.kind === "person" && <OwnedPage />}
          {shown === "resources" && route.kind === "resources" && me.role === "admin" && (
            <ResourcesPage tab={route.tab} onOpenDeployment={(slug) => navigate({ kind: "flow", slug, tab: "settings" })} />
          )}
          {shown === "people" && route.kind === "people" && me.role === "admin" && <PeoplePage me={me.name} tab={route.tab} />}
          {shown === "policy" && me.role === "admin" && <PolicyPage deployments={deployments} onOpenDeployment={(slug) => navigate({ kind: "flow", slug, tab: "status" })} />}
          {shown === "docs" && (
            <Suspense fallback={<div style={{ color: "var(--faint)" }}>loading the manual…</div>}>
              <ManualPage at={manualAt} onGo={setManualAt} onTry={openText} />
            </Suspense>
          )}
          {shown === "welcome" && (
            <WelcomePage
              name={me.name}
              showOnStart={start.prefs.welcome !== false}
              onShowOnStart={(show) => start.onPrefs({ ...start.prefs, welcome: show })}
              onAction={(a) => (a === "new" ? start.onNew() : setPage(a === "samples" ? "library" : a))}
              onManual={() => setPage("docs")}
              done={{ saved: savedFlows.length > 0, deployed: deployments.length > 0, ran: deployments.some((d) => d.trigger_count > 0) }}
              notes={start.prefs.notes ?? ""}
              onNotes={(notes) => start.onPrefs({ ...start.prefs, notes })}
            />
          )}
          {shown === "home" && <OverviewPage name={me.name} admin={me.role === "admin"} deployments={deployments} approvals={approvals.approvals} navigate={navigate} onNew={start.onNew} />}
          {shown === "flow" && (route.kind === "flow" || route.kind === "run") && (
            <FlowPage
              slug={route.slug}
              tab={route.kind === "flow" ? route.tab : "runs"}
              run={route.kind === "run" ? route.seq : null}
              entry={deployments.find((d) => d.slug === route.slug)}
              navigate={navigate}
              role={me.role}
              codeText={async (unit) => {
                const doc = await getDeployedDocument(route.slug, unit);
                const plain = printHolon(route.slug, doc, ev);
                return { plain, named: doc.bindings ? printHolon(route.slug, doc, ev, new Map(Object.entries(doc.bindings))) : undefined };
              }}
              onRestored={refreshDeployments}
              manifestOf={manifestOfDocument}
              card={{
                onAction: (action, from) => changeDeployment(route.slug, action, from).then(() => refreshDeployments()),
                onChanged: refreshDeployments,
                onRemove: async () => {
                  await deleteDeployment(route.slug);
                  refreshDeployments();
                  navigate({ kind: "flows" });
                },
                focus: focus?.slug === route.slug ? focus.n : undefined,
              }}
            />
          )}
        </div>
      )}
    </div>
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
        mails={derived.mails}
        feeds={derived.feeds}
        queues={derived.queues}
        sendsTo={sendsTo}
        websockets={derived.websockets}
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
  // What the work area shows: a page, or the canvas (the active flow's tab), as the address
  // says (routes.ts). A place shown by another page's is written as that page's.
  const { route, navigate, arrivedAt } = useHashRoute();
  const page = pageOf(route);
  const setPage = useCallback((p: AppPage) => navigate(routeOfPage(p)), [navigate]);
  const toast = useToast();
  const [waiting, setWaiting] = useState(0);
  const routeKey = format(route);
  useEffect(() => {
    const as = shownAt(route);
    if (as === null) return;
    if (route.kind === "notfound") toast(`Nothing is at #${route.hash}.`, "bad");
    navigate(as, { replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routeKey]);
  const routeNow = useRef(route);
  routeNow.current = route;

  // The keys that work anywhere (keymap.ts): a page by its number, Esc up, F6 across the panes,
  // and ? for them all. In the editor Esc stays the canvas's (and its overlays'): a run viewed
  // there is followed live, so going up from it would only be taken back.
  const [keysShown, setKeysShown] = useState(false);
  const toPage = (i: number) => () => {
    const to: Route[] = [
      { kind: "overview" },
      { kind: "approvals", tab: "deploys" },
      { kind: "flows" },
      { kind: "editor", slug: null, view: false, seq: null },
      { kind: "library", tab: "samples" },
      { kind: "resources", tab: "databases" },
      { kind: "people", tab: null },
      { kind: "policy" },
    ];
    navigate(to[i]);
  };
  useKeys({
    ...Object.fromEntries(PAGE_KEYS.map((_, i) => [String(i + 1), toPage(i)])),
    "?": () => setKeysShown(true),
    Escape: () => {
      if (route.kind === "editor") return false;
      const up = parentOf(route);
      if (!up) return false;
      navigate(up);
    },
    F6: () => movePane(1),
    "shift+F6": () => movePane(-1),
    t: () => onUi(ui === "terminal" ? "clean" : "terminal"),
  });
  // The command palette (ui/CommandPalette.tsx, its entries the canvas's): Ctrl+K, /, or the top bar.
  const [finding, setFinding] = useState(false);
  useKeys({ "ctrl+k": () => setFinding(true), "/": () => setFinding(true) }, { whileTyping: ["ctrl+k"] });
  // What the editor keeps for this person (home.ts): read once, saved as it changes. The welcome
  // opens on start unless they said not to; otherwise the editor starts at home.
  const [prefs, setPrefsState] = useState<Prefs>({});
  useEffect(() => {
    getPrefs()
      .then((raw) => {
        const p = readPrefs(raw);
        setPrefsState(p);
        // The account's look wins over this browser's.
        if (p.theme) applyTheme(p.theme);
        if (p.ui) applyUi(p.ui);
        if (p.welcome !== false) {
          // Shown only if they are still where the editor started, at no address of their
          // own: prefs that arrive after they opened something else don't take them away from
          // it, and a place they asked for is kept.
          if (arrivedAt === "" && routeNow.current.kind === "overview") navigate({ kind: "start" }, { replace: true });
        }
      })
      .catch(() => undefined);
  }, []);
  const onPrefs = useCallback((p: Prefs) => {
    setPrefsState(p);
    if (p.theme) applyTheme(p.theme);
    if (p.ui) applyUi(p.ui);
    setPrefs(p).catch(() => undefined);
  }, []);
  // The interface (look.ts): Clean, or Terminal; `t` switches, and says how to switch back.
  const ui = prefs.ui ?? storedUi();
  const onUi = (u: Ui) => {
    onPrefs({ ...prefs, ui: u });
    toast(u === "terminal" ? "Terminal interface: mono, square, keys on show. t to switch back." : "Clean interface. t for Terminal.");
  };
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

  // The canvas is shown by the new tab itself (useTabs's showTab), with it: shown first, the
  // address would name the tab before it, and be put back there.
  const onNew = () => setPendingNew((p) => ({ nonce: (p?.nonce ?? 0) + 1 }));

  return (
    <Shell
      route={route}
      page={page}
      navigate={navigate}
      me={me}
      onNew={onNew}
      onFind={() => setFinding(true)}
      onWelcome={() => setPage("welcome")}
      theme={prefs.theme ?? storedTheme()}
      onTheme={(theme) => onPrefs({ ...prefs, theme })}
      ui={ui}
      onUi={onUi}
      waiting={waiting}
    >
      {keysShown && <KeysDialog onClose={() => setKeysShown(false)} />}
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
                route={route}
                navigate={navigate}
                pendingNew={pendingNew}
                docs={docs}
                me={me}
                start={{ prefs, onPrefs, onNew, showKeys: () => setKeysShown(true), finding, onFinding: setFinding, onWaiting: setWaiting }}
              />
            )}
          </ReactFlowProvider>
        </div>
      )}
    </Shell>
  );
}

/** Whether the editor is as narrow as a phone (styles.css's 560 px). */
const phone = () => typeof window.matchMedia === "function" && window.matchMedia("(max-width: 560px)").matches;

/** The panes F6 moves between, in the page's order: the side nav, the top bar, the canvas, its
 *  side panes and the labelled regions of the page shown. */
const PANES = '.hl-nav, .hl-top, .hl-canvas__flow, [role="complementary"], [role="region"][aria-label]:not(.hl-toasts), main';

function movePane(by: -1 | 1) {
  const panes = [...document.querySelectorAll<HTMLElement>(PANES)].filter((p) => p.offsetParent !== null || p.getClientRects().length > 0);
  if (panes.length === 0) return;
  const at = panes.findIndex((p) => p.contains(document.activeElement));
  const next = panes[at < 0 ? (by > 0 ? 0 : panes.length - 1) : (at + by + panes.length) % panes.length];
  if (!next.hasAttribute("tabindex") && next.tabIndex < 0) next.tabIndex = -1;
  next.focus();
}

export default function App() {
  return (
    <DialogProvider>
      <ToastProvider>
        <AuthGate>{(me) => <Editor me={me} />}</AuthGate>
      </ToastProvider>
    </DialogProvider>
  );
}