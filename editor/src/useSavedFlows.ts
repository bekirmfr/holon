// Saving the open flow, and the saved flows around it: which there are and when each was saved
// (the palette offers them as nodes, and a holon placed from an older save offers an update),
// saving (asking first when other saved flows place this one and the save changes its ports),
// and putting a holon's newest save in place of an older one (asking first when that removes
// wires). holonUsage.ts says who places what.

import { useCallback, useEffect, useMemo, useState, type MutableRefObject } from "react";
import type { Edge, Viewport } from "@xyflow/react";
import { flowAsNode } from "./canvasOps";
import { replaceComposite } from "./composites";
import { documentUnits } from "./docUnits";
import { holonIndex, interfaceLoss } from "./holonUsage";
import { getFlow, listDeployments, listFlows, saveFlow, StaleSave, type FlowDocument } from "./server";
import { savedKeyOf } from "./tabs";
import { printHolon } from "./holonText";
import { flowToKeep, type KeptFlow } from "./keptFlow";
import type { DerivedServer } from "./canvasOps";
import type { Evaluator } from "./holon";
import type { FlowSettings } from "./DeploySettings";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { Notify } from "./ui/Toast";

type Dialogs = { confirm: (message: string, action?: string) => Promise<boolean>; prompt: (message: string, initial?: string) => Promise<string | null> };

export function useSavedFlows({
  refreshOn,
  flowName,
  setFlowName,
  nodes,
  edges,
  live,
  settings,
  derived,
  source,
  getViewport,
  ev,
  dialog,
  setNodes,
  setEdges,
  setSavedKey,
  setSavedOnServer,
  savedAt,
  setSavedAt,
  notify,
  setError,
}: {
  /** Read the saved flows again when this changes (the page shown, the last save). */
  refreshOn: unknown;
  flowName: string | null;
  setFlowName: (name: string) => void;
  nodes: PrimitiveNodeType[];
  edges: Edge[];
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  settings: FlowSettings;
  /** The server the canvas declares (canvasOps.deriveServer), its trace and unit, as of the last
   *  render (null before the first). */
  derived: () => DerivedServer | null;
  /** The text kept with the flow (the pane's, as typed), or null. */
  source: string | null;
  getViewport: () => Viewport;
  ev: Evaluator;
  dialog: Dialogs;
  setNodes: (update: (ns: PrimitiveNodeType[]) => PrimitiveNodeType[]) => void;
  setEdges: (es: Edge[]) => void;
  setSavedKey: (key: string) => void;
  setSavedOnServer: (saved: boolean) => void;
  /** The save the flow was opened from or last saved as (tabs.ts `FlowMeta.savedAt`). */
  savedAt: number | null;
  setSavedAt: (at: number | null) => void;
  /** Says what a save or an update did, once (ui/Toast.tsx). */
  notify: Notify;
  setError: (msg: string) => void;
}) {
  const [savedFlowTimes, setSavedFlowTimes] = useState<Map<string, number>>(new Map());
  useEffect(() => {
    if (refreshOn === null) return;
    listFlows()
      .then((fs) => setSavedFlowTimes(new Map(fs.map((f) => [f.name, f.saved_at_epoch_seconds]))))
      // Not read: what was read last stands (the deployments' read says the server isn't answering).
      .catch(() => undefined);
  }, [refreshOn]);
  const savedFlows = useMemo(() => [...savedFlowTimes.keys()], [savedFlowTimes]);

  // A flow other saved flows place as a node: they keep the version they placed, but a save that
  // changes its ports says so first, since updating them would remove wires.
  const placedElsewhereOk = useCallback(
    async (name: string, doc: FlowDocument) => {
      if (!savedFlowTimes.has(name)) return true;
      try {
        const [flows, deployed] = await Promise.all([listFlows(), listDeployments().catch(() => [])]);
        const users = (await holonIndex(flows, deployed, (n) => getFlow(n).then((r) => r.document))).get(name) ?? [];
        if (users.length === 0) return true;
        const previous = await getFlow(name);
        const before = flowAsNode(name, previous.document, ev, "before", { x: 0, y: 0 });
        const after = flowAsNode(name, doc, ev, "after", { x: 0, y: 0 });
        const loss = before.ok && after.ok ? interfaceLoss(before.node.data, after.node.data) : [];
        if (loss.length > 0) {
          const where = users.map((u) => (u.kind === "deployment" ? `${u.user} (deployed)` : u.user)).join(", ");
          return await dialog.confirm(
            `"${name}" is placed as a node in ${where}. This save changes its ports: ${loss.join("; ")}. They keep the version they placed; updating them would remove wires to those ports. Save anyway?`,
            "Save",
          );
        }
      } catch {
        // can't tell who uses it: save regardless, as before
      }
      return true;
    },
    [savedFlowTimes, ev, dialog],
  );

  // Saving answers its name and what it was saved as (tabs.ts's savedKey), or null when it wasn't.
  const onSave = useCallback(async (): Promise<{ name: string; key: string } | null> => {
    let name = flowName;
    // The save this replaces: the one it was opened from or last saved as, and none since (the
    // server refuses one made from another, so a colleague's or an agent's save isn't lost).
    let base = savedAt;
    if (!name) {
      name = await dialog.prompt("Flow name (letters, digits, - and _ only):");
      if (!name) return null;
      // A draft named now (New opens untitled) mustn't quietly replace a flow saved under that name.
      if (savedFlowTimes.has(name) && !(await dialog.confirm(`A flow named "${name}" is saved already. Replace it with this one?`, "Replace"))) return null;
      base = savedFlowTimes.get(name) ?? null;
      setFlowName(name);
    }
    let carried: ReturnType<typeof documentUnits>;
    try {
      carried = documentUnits(nodes.flatMap((n) => (n.data.body.kind === "Composite" ? [n.data.body.hash] : [])), ev);
    } catch (e) {
      notify(`not saved: ${e instanceof Error ? e.message : String(e)}`, "bad");
      return null;
    }
    const doc: FlowDocument = {
      nodes: nodes as unknown[],
      edges: edges as unknown[],
      viewport: getViewport(),
      ...carried,
      server: settings,
      ...(derived()?.trace ? { trace: derived()!.trace } : {}),
      ...(source !== null ? { source } : {}),
    };
    // Kept as its text (as typed, or the canvas printed) and where its nodes were moved: a canvas
    // its text doesn't spell isn't saved, as it isn't deployed (keptFlow.ts).
    let flow: KeptFlow & { document: FlowDocument };
    try {
      const text = source ?? printHolon(name, doc, ev);
      flow = { ...keptOrDraft(doc, text, derived()), document: doc };
    } catch (e) {
      notify(`not saved: ${e instanceof Error ? e.message : String(e)}`, "bad");
      return null;
    }
    if (!(await placedElsewhereOk(name, doc))) return null;
    const saved = name;
    const key = savedKeyOf(nodes, edges, settings, source);
    const kept = (entry: { saved_at_epoch_seconds: number }) => {
      setSavedKey(key);
      setSavedOnServer(true);
      setSavedAt(entry.saved_at_epoch_seconds);
      notify(`saved ${new Date(entry.saved_at_epoch_seconds * 1000).toLocaleTimeString()}`);
      return { name: saved, key };
    };
    const notKept = (e: unknown) => {
      notify(`not saved: ${e instanceof Error ? e.message : String(e)}`, "bad");
      return null;
    };
    try {
      return kept(await saveFlow(name, flow, base));
    } catch (e) {
      if (!(e instanceof StaleSave)) return notKept(e);
      // Saved since it was opened (or before, under a name given now), or removed: replaced only
      // when its author says so.
      const by = `${e.savedBy ? ` by ${e.savedBy}` : ""}, ${e.savedAt === null ? "" : new Date(e.savedAt * 1000).toLocaleString()}`;
      const what =
        e.savedAt === null ? `"${name}" was removed since you opened it.` : base === null ? `A flow named "${name}" is saved already${by}.` : `"${name}" was saved again${by}, since you opened it.`;
      const ask = e.savedAt === null ? "Save yours as it again?" : "Replace that save with yours? It is lost if you do.";
      if (!(await dialog.confirm(`${what} ${ask}`, e.savedAt === null ? "Save" : "Replace"))) return notKept(new Error(what));
      return saveFlow(name, flow, e.savedAt).then(kept, notKept);
    }
  }, [flowName, nodes, edges, getViewport, ev, settings, source, derived, dialog, savedFlowTimes, savedAt, placedElsewhereOk, setFlowName, notify, setSavedKey, setSavedOnServer, setSavedAt]);

  // Place the newest saved version of a holon over the one placed from an older save: same place,
  // and its wires kept to the ports the new version still has.
  const updateHolon = useCallback(
    (id: string) => {
      const old = live.current.nodes.find((n) => n.id === id);
      const from = old?.data.source;
      if (!old || !from) return;
      getFlow(from.flow)
        .then(async (rec) => {
          const placed = flowAsNode(from.flow, rec.document, ev, id, old.position, rec.saved_at_epoch_seconds);
          if (!placed.ok) return setError(placed.message);
          const body = placed.node.data.body;
          if (body.kind !== "Composite") return;
          // Wires to ports it no longer has (or now of a type they don't fit) would go: ask first.
          const { edges: kept, dropped } = replaceComposite(live.current.nodes, live.current.edges, id, { hash: body.hash, ...placed.node.data }, { inputs: new Map(), outputs: new Map() });
          if (dropped.length > 0) {
            const loss = interfaceLoss(old.data, placed.node.data);
            const ok = await dialog.confirm(
              `The newest "${from.flow}" changes ports wired here: ${loss.join("; ")}. Updating removes ${dropped.length} wire(s). Update anyway?`,
              "Update",
            );
            if (!ok) return;
          }
          setNodes((ns) => ns.map((n) => (n.id === id ? { ...placed.node, selected: n.selected } : n)));
          setEdges(kept);
          notify(`updated "${from.flow}" to its save of ${new Date(rec.saved_at_epoch_seconds * 1000).toLocaleString()}${dropped.length ? `; ${dropped.length} wire(s) to ports it no longer has removed` : ""}`);
        })
        .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    },
    [live, ev, setNodes, setEdges, notify, setError, dialog],
  );

  return { savedFlowTimes, savedFlows, onSave, updateHolon };
}

/** What a save keeps of `doc`: its text and layout (keptFlow.ts `flowToKeep`, which refuses a
 *  canvas its text doesn't spell); a canvas that doesn't derive a server yet (a draft) is kept as
 *  its text alone, laid out again when it is opened. */
function keptOrDraft(doc: FlowDocument, text: string, derived: DerivedServer | null): KeptFlow {
  if (!derived || derived.errors.length > 0 || !derived.serverHash) return { text, layout: null };
  return flowToKeep(doc, text, derived.serverHash);
}
