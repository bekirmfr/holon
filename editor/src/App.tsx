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
  variadicPrimitives,
  compatible,
  buildPreimage,
  Evaluator,
  type WUnit,
  type WValue,
  type WEdge,
  type WEncapSpec,
  type WCapability,
} from "./holon";
import {
  deployUnit,
  previewDeploy,
  changeDeployment,
  getDeployedDocument,
  type LogEntry,
  saveFlow,
  type FlowDocument,
  type FlowRecord,
} from "./server";
import { outputColor } from "./theme";
import { Palette, DND_MIME } from "./Palette";
import { PrimitiveNode, type PrimitiveData, type PrimitiveNodeType } from "./PrimitiveNode";
import { InspectPanel } from "./InspectPanel";
import { closureOf, documentKey, fromHex, structuralKey, toHex } from "./flowDoc";
import { History } from "./history";
import { DialogProvider, useDialog } from "./Dialog";
import { BottomPane, SidePane } from "./ViewPanes";
import { nodeExchanges, nodeTimings, recordedResponses, replay, stateAtStep, type ReplayNode } from "./replay";
import { ServerPanel } from "./ServerPanel";
import { DeployBar, DEFAULT_SETTINGS, type FlowSettings } from "./DeployBar";
import { isTemplate, templateDefaults, templateInterface, TEMPLATES } from "./templates";
import { findRoutes, routeSpec } from "./derive";
import { endpointUnit, serverUnit, type EndpointDraft } from "./endpoint";
import { isWEncapError, capabilityAcknowledgeName, describeItem } from "./diagnostics";
import { primitiveName, stageClosure, unitFor } from "./canvasUnits";
import { excludeProjectedOutput, expandComposite, groupSelection, projectCanvas, type Projection } from "./canvasOps";
import { FlowToolbar, ProjectionPanel } from "./CanvasPanels";
import { DeployedPage, FlowsPage, type AppPage } from "./Pages";
import { newHistory, snapshotOf, useCanvasHistory, type Snapshot } from "./useCanvasHistory";
import { useDeployments } from "./useDeployments";

// What a save records, for unsaved-change tracking: the canvas plus the server draft.
const savedKeyOf = (ns: PrimitiveNodeType[], es: Edge[], settings: FlowSettings) =>
  documentKey(ns, es) + JSON.stringify(settings);

const nodeTypes = { primitive: PrimitiveNode } as unknown as NodeTypes;

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
  // View mode: a deployed flow's canvas, read-only, with its live flow log (onView). The flow
  // being edited is set aside meanwhile and comes back on exit, unsaved changes included.
  const [viewing, setViewing] = useState<string | null>(null);
  const [selectedRun, setSelectedRun] = useState<LogEntry | null>(null);
  const [step, setStep] = useState<number | null>(null);
  // The deployed flow's map from a run's steps to canvas nodes, and the node the side pane shows.
  const [viewTrace, setViewTrace] = useState<Record<string, string[]> | undefined>(undefined);
  const [inspectNode, setInspectNode] = useState<string | null>(null);
  const beforeView = useRef<{
    nodes: PrimitiveNodeType[];
    edges: Edge[];
    viewport: { x: number; y: number; zoom: number };
    history: History<Snapshot>;
    dirty: boolean;
  } | null>(null);
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
  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  // "New" from the menu: a clean canvas under the prompted name. `nonce` makes a second New
  // with the same name still fire.
  useEffect(() => {
    if (!pendingNew) return;
    setViewing(null);
    beforeView.current = null;
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
    bump();
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
      // Opening the flow being edited only reloads it when asked to ("reload saved"), and still
      // asks first; FlowsPage resumes it instead (openChoice).
      const discard =
        rec.name === flowName
          ? `Discard your unsaved changes to "${rec.name}" and reload the saved copy?`
          : `Discard unsaved changes to "${flowName ?? "untitled"}" and open "${rec.name}"?`;
      if (dirty && !(await dialog.confirm(discard, rec.name === flowName ? "Reload saved" : "Discard"))) {
        return;
      }
      setViewing(null);
      beforeView.current = null;
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
        ...(rec.document.server?.requirements ? { requirements: rec.document.server.requirements } : {}),
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
      bump();
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
      if (viewing) return;
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
    [byName, screenToFlowPosition, setNodes, viewing],
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

  // Which canvas node each child of a frozen unit is, in the order the unit is stored (and run,
  // and timed) in: core sorts a composite's children canonically when it hashes it.
  const traceOf = useCallback(
    (unit: WUnit, group: PrimitiveNodeType[]) =>
      unit.body.kind === "Composite"
        ? Array.from(ev.canonical_child_order(unit.body.children, unit.body.edges), (i) => group[i].id)
        : [],
    [ev],
  );

  // The server this flow declares, derived from the canvas: each connected group holding a
  // request node is one route (derive.ts). Each group is projected through core like a
  // selection being grouped, narrowed to the request input and the response/effect outputs,
  // and wrapped as an endpoint. Recomputed when the canvas's structure changes, not on moves.
  const derived = useMemo(() => {
    const { nodes: ns, edges: es } = live.current;
    const { routes, errors } = findRoutes(ns, es);
    const endpoints: EndpointDraft[] = [];
    const trace: Record<string, string[]> = {};
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
        trace[label] = traceOf(handler, group);
        const handlerHash = ev.insert(buildPreimage(handler));
        const unit = endpointUnit(handlerHash, handler.inputs ?? [], handler.outputs ?? [], r.method, r.path, r.resume);
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
    return { endpoints, errors, serverHash, trace };
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canvasKey, ev, byName, groupUnits, traceOf, settings.port]);

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
  const { deployments, refresh: refreshDeployments } = deploymentTable;
  const [deployBusy, setDeployBusy] = useState(false);

  // ── Deploy: one action for the whole flow, named after it (DeployBar in the header) ──────
  // A flow with routes deploys as its derived server; any other flow as the whole canvas
  // projected into one unit (a clock or ingress flow). Before anything is sent, the person sees
  // exactly what it may touch (the server's consent gate checks the same list), in the dialog.
  const deployedEntry = flowName ? deployments.find((d) => d.slug === flowName) : undefined;
  const outOfDate = deployedEntry && derived.serverHash ? deployedEntry.unit !== derived.serverHash : null;
  // The secrets the canvas reads, by name (its `secret` nodes): what ⚙ lets you set.
  const secretNames = useMemo(
    () =>
      [
        ...new Set(
          nodes.flatMap((n) =>
            n.data.name === "secret" && n.data.body.kind === "Primitive"
              ? n.data.params.flatMap((p) => (p.name === "name" && p.kind === "Frozen" ? [p.value] : []))
              : [],
          ),
        ),
      ].sort(),
    [nodes],
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
    const caps = (ev.capabilities(hash) as WCapability[]).map(capabilityAcknowledgeName);
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
      const lines = [`${existing ? "Update" : "Deploy"} "${name}"?`];
      if (ask.length > 0) lines.push("", "It needs your approval to:", ...ask.map((i) => `• ${describeItem(i)}`));
      if (preview.already_granted.length > 0) {
        lines.push("", "Already granted:", ...preview.already_granted.map((i) => `• ${describeItem(i)}`));
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
      const units = closureOf(
        ns.flatMap((n) => (n.data.body.kind === "Composite" ? [n.data.body.hash] : [])),
        (h) => ev.get_unit(h) as WUnit,
      ).map((h) => toHex(buildPreimage(ev.get_unit(h) as WUnit)));
      const document: FlowDocument = { nodes: ns, edges: es, viewport: getViewport(), units, server: settings, trace };
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
  const onView = useCallback(
    async (slug: string) => {
      let doc: FlowDocument;
      try {
        doc = await getDeployedDocument(slug);
      } catch (e) {
        setDeployError(e instanceof Error ? e.message : String(e));
        return;
      }
      if (!viewing) {
        beforeView.current = {
          nodes: live.current.nodes,
          edges: live.current.edges,
          viewport: getViewport(),
          history: history.current,
          dirty,
        };
      }
      for (const hex of doc.units ?? []) {
        try {
          ev.insert(fromHex(hex));
        } catch {
          // a unit that won't decode just stays unresolvable, like on Open
        }
      }
      history.current = newHistory(snapshotOf([], []));
      restoring.current = true;
      setNodes(doc.nodes as PrimitiveNodeType[]);
      setEdges(doc.edges as Edge[]);
      setViewport(doc.viewport);
      setViewing(slug);
      setViewTrace(doc.trace);
      setInspectNode(null);
      setSelectedRun(null);
      setStep(null);
      resetProjectionView();
      setPage("canvas");
    },
    [viewing, getViewport, dirty, ev, snapshotOf, setNodes, setEdges, setViewport, resetProjectionView, setPage],
  );

  const onExitView = useCallback(() => {
    const before = beforeView.current;
    beforeView.current = null;
    setViewing(null);
    setViewTrace(undefined);
    setInspectNode(null);
    setSelectedRun(null);
    setStep(null);
    if (before) {
      history.current = before.history;
      restoring.current = true;
      setNodes(before.nodes);
      setEdges(before.edges);
      setViewport(before.viewport);
    }
  }, [setNodes, setEdges, setViewport]);

  // "open" on the flow already being edited: back to the canvas, unsaved changes and all (out of
  // a deployed flow's view, if one is shown over it).
  const onResumeFlow = useCallback(() => {
    if (viewing) onExitView();
    setPage("canvas");
  }, [viewing, onExitView, setPage]);

  // One node of a replay: its unit, evaluated on the inputs its wires brought.
  const evalNode = useCallback(
    (node: ReplayNode, inputs: (WValue | null)[]): (WValue | null)[] => {
      try {
        const hash = unitFor(node as PrimitiveNodeType, ev, byName);
        return ev.eval(buildPreimage(ev.get_unit(hash) as WUnit), inputs) as (WValue | null)[];
      } catch {
        return node.data.outputs.map(() => null);
      }
    },
    [ev, byName],
  );

  // The selected run, replayed on the canvas; and whether that reproduced what the server sent.
  const replayed = useMemo(() => {
    if (!viewing || !selectedRun) return null;
    const { nodes: ns, edges: es } = live.current;
    // Outbound requests are answered from the run's record, never the network.
    const run = replay(ns, es, selectedRun, evalNode, recordedResponses(nodeExchanges(selectedRun, viewTrace)));
    let verdict: boolean | null = null;
    const respond = run.order.find((id) => {
      const n = ns.find((x) => x.id === id);
      return n?.data.name === "respond" && n.data.body.kind === "Template";
    });
    if (selectedRun.kind === "request" && respond && selectedRun.outputs.length > 0) {
      verdict = JSON.stringify(run.traces.get(respond)?.outputs[0] ?? null) === JSON.stringify(selectedRun.outputs[0]);
    }
    return { ...run, verdict };
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing, selectedRun, canvasKey, evalNode, viewTrace]);

  const onSelectRun = useCallback((e: LogEntry | null) => {
    setSelectedRun(e);
    setStep(null);
  }, []);

  // Stepping through a run follows the node at that step in the side pane.
  const onStep = useCallback(
    (s: number | null) => {
      setStep(s);
      const id = s === null ? undefined : replayed?.order[s];
      if (id) setInspectNode(id);
    },
    [replayed],
  );
  const onNodeClick = useCallback(
    (_: unknown, node: PrimitiveNodeType) => {
      if (viewing) setInspectNode(node.id);
    },
    [viewing],
  );

  // When each node of the selected run ran, and the flow state as of the current step.
  const timings = useMemo(
    () => (selectedRun ? nodeTimings(selectedRun, viewTrace) : new Map<string, { start_us: number; end_us: number }>()),
    [selectedRun, viewTrace],
  );
  const exchanges = useMemo(() => (selectedRun ? nodeExchanges(selectedRun, viewTrace) : []), [selectedRun, viewTrace]);
  const flowState = useMemo(
    () =>
      selectedRun && replayed
        ? stateAtStep(selectedRun.state_before, replayed, live.current.nodes, step)
        : { after: {}, changed: [] as string[] },
    [selectedRun, replayed, step],
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
  const onExpand = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length !== 1) return; // mirrors the button's own disabled guard, defensively
    const target = selected[0];
    if (target.data.body.kind !== "Composite") return;
    const result = expandComposite(target, edges, ev, byName, variadic, () => idRef.current++);
    if (result.ok) {
      setNodes((ns) => [...ns.filter((n) => n.id !== target.id), ...result.nodes]);
      setEdges(result.edges);
      setProjError("");
      setErrorNodeIds(new Set());
    } else {
      setProjError(result.message);
    }
  }, [nodes, edges, ev, byName, variadic, setNodes, setEdges]);

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
  const decoratedNodes = useMemo(() => {
    if (viewing) {
      const order = replayed?.order ?? [];
      return nodes.map((n) => {
        const i = order.indexOf(n.id);
        const state: "active" | "done" | "pending" | "outside" =
          i < 0 ? "outside" : step === null || i < step ? "done" : i === step ? "active" : "pending";
        const trace = replayed ? { outputs: replayed.traces.get(n.id)?.outputs ?? [], state } : undefined;
        return { ...n, data: { ...n.data, readOnly: true, trace } };
      });
    }
    return errorNodeIds.size === 0
      ? nodes
      : nodes.map((n) => (errorNodeIds.has(n.id) ? { ...n, data: { ...n.data, error: true } } : n));
  }, [nodes, errorNodeIds, viewing, replayed, step]);

  // How many placed nodes are currently selected — React Flow's own selection state
  // (shift-click / drag-box), already live for delete, now also gating the "group" button
  // (P4.3.1). Cheap enough over a canvas-sized node list not to need memoizing.
  const selectedCount = nodes.filter((n) => n.selected).length;
  // "Expand" is the dual of "group": only meaningful for exactly one selected composite node —
  // mirrors group's own selectedCount<2 guard rather than a new per-node interaction
  // PrimitiveNode.tsx doesn't otherwise have for whole-node operations.
  const canExpand = selectedCount === 1 && nodes.some((n) => n.selected && n.data.body.kind === "Composite");

  return (
    <div className={viewing ? "hl-canvas hl-canvas--viewing" : "hl-canvas"} onDrop={onDrop} onDragOver={onDragOver}>
      <div className="hl-canvas__flow">
        <ReactFlow<PrimitiveNodeType>
          nodes={decoratedNodes}
          onNodeClick={onNodeClick}
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
          deleteKeyCode={viewing ? null : ["Backspace", "Delete"]}
          nodesDraggable={!viewing}
          nodesConnectable={!viewing}
        >
          <Background variant={BackgroundVariant.Lines} gap={26} color="#1b2a45" />
          <Controls showInteractive={false} />
          <Panel position="top-left">
            <FlowToolbar
              histVersion={histVersion}
              viewing={viewing}
              onExitView={onExitView}
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
                />
              }
              saveMsg={saveMsg}
              error={page === "canvas" ? deployError : ""}
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
      </div>
      {viewing && (
        <>
          <SidePane
            run={selectedRun}
            step={step}
            order={replayed?.order ?? []}
            stateAfter={flowState.after}
            changed={flowState.changed}
            node={nodes.find((n) => n.id === inspectNode) ?? null}
            trace={inspectNode ? replayed?.traces.get(inspectNode) : undefined}
            timing={inspectNode ? timings.get(inspectNode) : undefined}
          />
          <BottomPane
            slug={viewing}
            selected={selectedRun}
            onSelect={onSelectRun}
            step={step}
            onStep={onStep}
            order={replayed?.order ?? []}
            nodes={nodes}
            timings={timings}
            exchanges={exchanges}
            verdict={replayed?.verdict ?? null}
            onPickNode={setInspectNode}
          />
        </>
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
          {page === "flows" && (
            <FlowsPage current={{ name: flowName, dirty }} onOpen={onOpenFlow} onResume={onResumeFlow} />
          )}
          {page === "deployed" && (
            <DeployedPage
              deployments={deployments}
              error={deployError}
              onAction={(slug, action, from) => changeDeployment(slug, action, from).then(() => refreshDeployments())}
              triggerInputs={deploymentTable.triggerInputs}
              onTriggerInputChange={deploymentTable.onTriggerInputChange}
              onTrigger={deploymentTable.onTrigger}
              triggerResults={deploymentTable.triggerResults}
              onDelete={deploymentTable.onDelete}
              onView={onView}
            />
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