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
import { holonUses, interfaceLoss } from "./holonUsage";
import { getAllFlows, getFlow, listFlows, saveFlow, type FlowDocument } from "./server";
import { savedKeyOf } from "./tabs";
import type { Evaluator, WUnit } from "./holon";
import type { FlowSettings } from "./DeployBar";
import type { PrimitiveNodeType } from "./PrimitiveNode";

type Dialogs = { confirm: (message: string, action?: string) => Promise<boolean>; prompt: (message: string, initial?: string) => Promise<string | null> };

export function useSavedFlows({
  refreshOn,
  flowName,
  setFlowName,
  nodes,
  edges,
  live,
  settings,
  getViewport,
  ev,
  byName,
  dialog,
  setNodes,
  setEdges,
  setSavedKey,
  setSavedOnServer,
  setSaveMsg,
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
  getViewport: () => Viewport;
  ev: Evaluator;
  byName: Map<string, WUnit>;
  dialog: Dialogs;
  setNodes: (update: (ns: PrimitiveNodeType[]) => PrimitiveNodeType[]) => void;
  setEdges: (es: Edge[]) => void;
  setSavedKey: (key: string) => void;
  setSavedOnServer: (saved: boolean) => void;
  setSaveMsg: (msg: string) => void;
  setError: (msg: string) => void;
}) {
  const [savedFlowTimes, setSavedFlowTimes] = useState<Map<string, number>>(new Map());
  useEffect(() => {
    if (refreshOn === null) return;
    listFlows()
      .then((fs) => setSavedFlowTimes(new Map(fs.map((f) => [f.name, f.saved_at_epoch_seconds]))))
      .catch(() => setSavedFlowTimes(new Map()));
  }, [refreshOn]);
  const savedFlows = useMemo(() => [...savedFlowTimes.keys()], [savedFlowTimes]);

  // A flow other saved flows place as a node: they keep the version they placed, but a save that
  // changes its ports says so first, since updating them would remove wires.
  const placedElsewhereOk = useCallback(
    async (name: string, doc: FlowDocument) => {
      if (!savedFlowTimes.has(name)) return true;
      try {
        const all = await getAllFlows();
        const users = holonUses(all).get(name) ?? [];
        const previous = all.find((r) => r.name === name);
        const before = previous && flowAsNode(name, previous.document, ev, byName, "before", { x: 0, y: 0 });
        const after = flowAsNode(name, doc, ev, byName, "after", { x: 0, y: 0 });
        const loss = before?.ok && after.ok ? interfaceLoss(before.node.data, after.node.data) : [];
        if (users.length > 0 && loss.length > 0) {
          return await dialog.confirm(
            `"${name}" is placed as a node in ${users.map((u) => u.user).join(", ")}. This save changes its ports: ${loss.join("; ")}. They keep the version they placed; updating them would remove wires to those ports. Save anyway?`,
            "Save",
          );
        }
      } catch {
        // can't tell who uses it: save regardless, as before
      }
      return true;
    },
    [savedFlowTimes, ev, byName, dialog],
  );

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
    if (!(await placedElsewhereOk(name, doc))) return;
    const key = savedKeyOf(nodes, edges, settings);
    saveFlow(name, doc)
      .then((entry) => {
        setSavedKey(key);
        setSavedOnServer(true);
        setSaveMsg(`saved ${new Date(entry.saved_at_epoch_seconds * 1000).toLocaleTimeString()}`);
      })
      .catch((e: unknown) => setSaveMsg(e instanceof Error ? e.message : String(e)));
  }, [flowName, nodes, edges, getViewport, ev, settings, dialog, placedElsewhereOk, setFlowName, setSaveMsg, setSavedKey, setSavedOnServer]);

  // Place the newest saved version of a holon over the one placed from an older save: same place,
  // and its wires kept to the ports the new version still has.
  const updateHolon = useCallback(
    (id: string) => {
      const old = live.current.nodes.find((n) => n.id === id);
      const from = old?.data.source;
      if (!old || !from) return;
      getFlow(from.flow)
        .then(async (rec) => {
          const placed = flowAsNode(from.flow, rec.document, ev, byName, id, old.position, rec.saved_at_epoch_seconds);
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
          setSaveMsg(`updated "${from.flow}" to its save of ${new Date(rec.saved_at_epoch_seconds * 1000).toLocaleString()}${dropped.length ? `; ${dropped.length} wire(s) to ports it no longer has removed` : ""}`);
        })
        .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    },
    [live, ev, byName, setNodes, setEdges, setSaveMsg, setError, dialog],
  );

  return { savedFlowTimes, savedFlows, onSave, updateHolon };
}
