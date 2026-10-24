// The text pane and the canvas, one flow (docs/L4s/text-pane-L4.md). The last edit wins: what is
// typed drives the canvas when it compiles (one undo step; nodes it didn't touch keep their place
// and id, textSync.ts), and a canvas change writes the text again from the canvas. Moving a node
// changes nothing in the text: a position is not part of the program. The text as typed is kept
// with the tab (FlowMeta's `text`) and saved with the flow while it stands for the canvas, so its
// comments survive; a canvas change that drops comments says so, and undoing it brings them back.

import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import type { Edge } from "@xyflow/react";
import type { Evaluator } from "./holon";
import type { FlowSettings } from "./DeployBar";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { compileHolon, printHolon } from "./holonText";
import { carryOver, commentCount, syncedFromCanvas, type Graph, type Synced } from "./textSync";
import { settingsOf, syncKeyOf, type FlowMeta, type TextState } from "./tabs";

const WIDTH_KEY = "holon.textWidth";
/** How long typing pauses before the text is compiled: for the person, not the machine. */
const PAUSE_MS = 300;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function useTextPane({
  flow,
  setFlow,
  nodes,
  edges,
  canvasKey,
  live,
  setNodes,
  setEdges,
  ev,
  idRef,
  viewing,
}: {
  flow: FlowMeta;
  setFlow: (update: (m: FlowMeta) => FlowMeta) => void;
  nodes: PrimitiveNodeType[];
  edges: Edge[];
  /** The canvas's structure (flowDoc.ts's structuralKey): it changes when the structure does. */
  canvasKey: string;
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  setNodes: (update: PrimitiveNodeType[] | ((ns: PrimitiveNodeType[]) => PrimitiveNodeType[])) => void;
  setEdges: (es: Edge[]) => void;
  ev: Evaluator;
  idRef: MutableRefObject<number>;
  viewing: boolean;
}) {
  const text = flow.text;
  const setText = useCallback((update: (t: TextState) => TextState) => setFlow((m) => ({ ...m, text: update(m.text) })), [setFlow]);
  const [status, setStatus] = useState("");
  const [width, setWidthState] = useState(() => {
    try {
      return Number(localStorage.getItem(WIDTH_KEY)) || 420;
    } catch {
      return 420;
    }
  });
  const setWidth = useCallback((w: number) => {
    setWidthState(w);
    try {
      localStorage.setItem(WIDTH_KEY, String(Math.round(w)));
    } catch {
      // Not remembered, then.
    }
  }, []);

  // What the text stands for, as of this render.
  const key = useMemo(
    () => syncKeyOf(live.current.nodes, live.current.edges, flow.settings, flow.name),
    // canvasKey changes exactly when the structure does; live.current is read on purpose.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [canvasKey, flow.settings, flow.name],
  );
  const now = useRef({ flow, key });
  now.current = { flow, key };

  // Texts as typed, by the canvas they stood for: undoing a canvas change brings its text back.
  const typedTexts = useRef(new Map<string, string>());
  const remember = (k: string, t: string) => {
    typedTexts.current.set(k, t);
    if (typedTexts.current.size > 100) typedTexts.current.delete(typedTexts.current.keys().next().value!);
  };
  // The compiled text that stands for the canvas, and which canvas node each of its nodes is.
  const synced = useRef<{ key: string; synced: Synced; lines: Map<string, number> } | null>(null);

  /** The canvas as a document to print or match. */
  const canvasDoc = () => ({ nodes: live.current.nodes as unknown[], edges: live.current.edges as unknown[], server: now.current.flow.settings });

  /** The sync for the canvas as it is: from the text that stands for it, else the canvas printed. */
  const syncedNow = useCallback(() => {
    const { flow: f, key: k } = now.current;
    if (synced.current?.key === k) return synced.current;
    const base = f.text.key === k && f.text.synced !== null ? f.text.synced : printHolon(f.name, canvasDoc(), ev);
    const c = compileHolon(base, ev);
    const canvas: Graph = { nodes: live.current.nodes, edges: live.current.edges };
    synced.current = { key: k, synced: syncedFromCanvas(canvas, { nodes: c.doc.nodes, edges: c.doc.edges } as Graph, c.names), lines: c.lines };
    return synced.current;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ev]);

  // The canvas changed (or the pane opened on it): the text is written again from it, or the
  // text typed for it before comes back.
  useEffect(() => {
    if (!text.open || viewing || text.key === key) return;
    const remembered = typedTexts.current.get(key);
    let value: string | null = remembered ?? null;
    if (value === null) {
      try {
        value = printHolon(flow.name, canvasDoc(), ev);
      } catch (e) {
        setStatus(`the canvas can't be written as text yet: ${message(e)}`);
      }
    }
    const before = text.typed ? text.value : null;
    const dropped = before !== null && value !== null && remembered === undefined ? commentCount(before) - commentCount(value) : 0;
    if (value !== null) setStatus(dropped > 0 ? `The canvas change wrote the text again: ${dropped === 1 ? "a comment was" : `${dropped} comments were`} dropped. Undo brings ${dropped === 1 ? "it" : "them"} back.` : "");
    synced.current = null;
    setText((t) => ({ ...t, value, key, synced: value, typed: remembered !== undefined }));
    // Only a new canvas (or opening) writes the text; the text's own fields follow from it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text.open, viewing, key]);

  // What was typed, compiled after a pause: the canvas it stands for, with what it didn't touch
  // where it was.
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const apply = useCallback(
    (typed: string) => {
      const { flow: f, key: k } = now.current;
      let compiled: ReturnType<typeof compileHolon>;
      try {
        compiled = compileHolon(typed, ev);
      } catch (e) {
        setStatus(`${message(e)}. The canvas shows the last version that compiled.`);
        return;
      }
      let prev: Synced;
      try {
        prev = syncedNow().synced;
      } catch {
        prev = { graph: { nodes: [], edges: [] }, names: new Map(), toCanvas: new Map() };
      }
      const next = { graph: { nodes: compiled.doc.nodes, edges: compiled.doc.edges } as Graph, names: compiled.names };
      const out = carryOver(prev, next, live.current.nodes, (base) => `${base}-${idRef.current++}`);
      // A node kept is the canvas's own (its selection, its measured size), with the text's data.
      const old = new Map(live.current.nodes.map((n) => [n.id, n]));
      const ns = out.nodes.map((n) => {
        const o = old.get(n.id);
        return o ? { ...o, data: n.data, position: n.position } : n;
      });
      // The settings the text says (port, every, browsers, optional); the limits it doesn't say
      // are the flow's as they were.
      const said = settingsOf(compiled.doc.server);
      const { optional: _, ...unsaid } = f.settings.requirements ?? {};
      const optional = said.requirements?.optional;
      const requirements = { ...unsaid, ...(optional && optional.length > 0 ? { optional } : {}) };
      const settings: FlowSettings = { port: said.port, interval: said.interval, ...(Object.keys(requirements).length > 0 ? { requirements } : {}), ...(said.browsers ? { browsers: said.browsers } : {}) };
      const k2 = syncKeyOf(ns, out.edges, settings, f.name);
      synced.current = { key: k2, synced: { ...next, toCanvas: out.toCanvas }, lines: compiled.lines };
      remember(k2, typed);
      setStatus("");
      if (k2 === k) {
        // Only comments or spacing changed: the canvas is as it was.
        setText((t) => ({ ...t, key: k2, synced: typed, typed: true }));
        return;
      }
      setNodes(ns);
      setEdges(out.edges);
      setFlow((m) => ({ ...m, settings, text: { ...m.text, key: k2, synced: typed, typed: true } }));
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ev, syncedNow, setNodes, setEdges, setFlow, setText],
  );

  const onEdit = useCallback(
    (typed: string, formatted: boolean) => {
      setText((t) => ({ ...t, value: typed, typed: true }));
      if (timer.current) clearTimeout(timer.current);
      // Formatted, nothing is half typed: no pause to wait for.
      if (formatted) apply(typed);
      else timer.current = setTimeout(() => apply(typed), PAUSE_MS);
    },
    [apply, setText],
  );
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);

  // In view mode: the deployed version's text, read only.
  const viewText = useMemo(() => {
    if (!viewing || !text.open) return null;
    try {
      return printHolon(flow.name, { nodes: nodes as unknown[], edges: edges as unknown[], server: flow.settings }, ev);
    } catch (e) {
      return `# the deployed version can't be written as text: ${message(e)}\n`;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing, text.open, canvasKey, flow.name, ev]);

  // The lines of what is selected on the canvas, in the text as shown (while it stands for it).
  const selectedIds = nodes.filter((n) => n.selected).map((n) => n.id).join(" ");
  const lines = useMemo(() => {
    if (!text.open || viewing || selectedIds === "" || text.key !== key || text.value !== text.synced) return [];
    try {
      const s = syncedNow();
      const toText = new Map([...s.synced.toCanvas].map(([t, c]) => [c, t]));
      return selectedIds.split(" ").flatMap((id) => {
        const l = s.lines.get(toText.get(id) ?? "");
        return l === undefined ? [] : [l];
      });
    } catch {
      return [];
    }
  }, [text.open, viewing, selectedIds, text.key, text.value, text.synced, key, syncedNow]);

  // The cursor on a line selects the nodes written on it.
  const onCursorLine = useCallback(
    (line: number) => {
      const { flow: f, key: k } = now.current;
      if (f.text.key !== k || f.text.value !== f.text.synced) return;
      let s: ReturnType<typeof syncedNow>;
      try {
        s = syncedNow();
      } catch {
        return;
      }
      const ids = new Set([...s.lines].filter(([, l]) => l === line).map(([t]) => s.synced.toCanvas.get(t)).filter((c): c is string => c !== undefined));
      if (ids.size === 0) return;
      setNodes((ns) => ns.map((n) => (Boolean(n.selected) === ids.has(n.id) ? n : { ...n, selected: ids.has(n.id) })));
    },
    [syncedNow, setNodes],
  );

  const toggle = useCallback(() => setText((t) => ({ ...t, open: !t.open })), [setText]);

  return {
    open: text.open,
    toggle,
    pane: {
      text: viewing ? (viewText ?? "") : (text.value ?? ""),
      readOnly: viewing,
      onEdit,
      lines,
      onCursorLine,
      status: viewing ? "" : status,
      width,
      onWidth: setWidth,
      onClose: toggle,
    },
  };
}
